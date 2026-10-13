//! Knowledge store: this workspace's own emails, attachment text and approved/sent replies,
//! chunked and searchable by keywords (FTS5 BM25) and meaning (sqlite-vec KNN), merged by
//! reciprocal rank fusion. Every query is scoped to one workspace.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    ai::{
        AiError, LabelExample, ReplyExample,
        embeddings::{EMBEDDING_DIMS, Embedder, to_blob},
        prepare_text,
    },
    db::now_ms,
    sources::normalize::truncate_chars,
};

/// Target chunk size and overlap, in characters (~200 tokens per chunk).
pub const CHUNK_CHARS: usize = 800;
pub const CHUNK_OVERLAP: usize = 120;
/// Per-document cap so one long thread or CV can't dominate cost or results.
pub const MAX_DOC_CHARS: usize = 8_000;
pub const MAX_ATTACHMENTS: i64 = 5;
const MAX_QUERY_CHARS: usize = 2_000;
const MAX_FTS_TERMS: usize = 24;
/// Standard RRF constant: dampens the advantage of top ranks so both lists count.
pub const RRF_K: f64 = 60.0;
/// Candidates taken from each leg before fusion when retrieving examples.
const EXAMPLE_CANDIDATES: usize = 40;
pub const MAX_LABEL_EXAMPLES: usize = 5;
pub const MAX_REPLY_EXAMPLES: usize = 3;
/// Auto labels at or above this confidence (and not `other`) count as trustworthy examples.
pub const TRUSTED_CONFIDENCE: f64 = 0.9;
/// Chunks embedded per provider round inside one job.
const EMBED_ROUND: i64 = 64;
const MAX_EMBED_ROUNDS: usize = 4;

pub mod kind {
    pub const EMAIL: &str = "email";
    pub const ATTACHMENT: &str = "attachment";
    pub const REPLY: &str = "reply";
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Ai(#[from] AiError),
}

// ---------------------------------------------------------------- chunking

fn is_reply_header(line: &str) -> bool {
    line.starts_with("-----Original Message-----") || (line.starts_with("On ") && line.ends_with("wrote:"))
}

/// Trims lines, collapses inner whitespace and runs of blank lines.
pub fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank = true;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            if !blank {
                out.push('\n');
            }
            blank = true;
            continue;
        }
        out.push_str(&line);
        out.push('\n');
        blank = false;
    }
    out.trim().to_string()
}

/// The new part of an email: quoted history (`>` lines, "On … wrote:") is dropped.
pub fn clean_body(text: &str) -> String {
    let mut kept = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if is_reply_header(t) {
            break;
        }
        if !t.starts_with('>') {
            kept.push(t);
        }
    }
    tidy(&kept.join("\n"))
}

/// Splits on whitespace near `size` characters; consecutive chunks share ~`overlap`
/// characters, starting on a word boundary. UTF-8 safe.
pub fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < n {
        let mut end = (start + size).min(n);
        if end < n
            && let Some(ws) = (start + size / 2..end).rev().find(|&i| chars[i].is_whitespace())
        {
            end = ws;
        }
        let chunk = chars[start..end].iter().collect::<String>().trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        if end >= n {
            break;
        }
        let mut next = end.saturating_sub(overlap).max(start + 1);
        while next < end && !chars[next - 1].is_whitespace() {
            next += 1;
        }
        start = next;
    }
    chunks
}

/// One indexed source (an email body, an attachment, a reply), replaced as a unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub kind: &'static str,
    pub ref_id: String,
    pub chunks: Vec<String>,
}

fn titled_chunks(title: &str, body: &str) -> Vec<String> {
    let title = truncate_chars(title.trim(), 200);
    let mut chunks: Vec<String> = chunk_text(body, CHUNK_CHARS, CHUNK_OVERLAP)
        .into_iter()
        .map(|c| prepare_text(&format!("{title}\n{c}")))
        .collect();
    if chunks.is_empty() && !title.is_empty() {
        chunks.push(prepare_text(&title));
    }
    chunks
}

/// Every chunk starts with the subject line, so each one carries its topic.
pub fn email_document(message_id: &str, subject: &str, body: &str) -> Document {
    let body = truncate_chars(&clean_body(body), MAX_DOC_CHARS);
    Document { kind: kind::EMAIL, ref_id: message_id.into(), chunks: titled_chunks(subject, &body) }
}

pub fn attachment_document(attachment_id: &str, filename: &str, text: &str) -> Document {
    let body = truncate_chars(&tidy(text), MAX_DOC_CHARS);
    Document { kind: kind::ATTACHMENT, ref_id: attachment_id.into(), chunks: titled_chunks(filename, &body) }
}

/// A reply plus a short excerpt of the email it answered, so a new similar email finds it.
pub fn reply_document(draft_id: &str, subject: &str, email_body: &str, reply: &str) -> Document {
    let excerpt = truncate_chars(&clean_body(email_body), 300);
    let text = format!("{}\n{excerpt}\n---\n{}", subject.trim(), tidy(reply));
    Document { kind: kind::REPLY, ref_id: draft_id.into(), chunks: vec![prepare_text(&truncate_chars(&text, 2_000))] }
}

pub fn content_hash(chunks: &[String]) -> String {
    let mut h = Sha256::new();
    for c in chunks {
        h.update(c.as_bytes());
        h.update([0x1f]);
    }
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------- writes

/// Replaces a document's chunks unless its content is unchanged. Returns true if written.
/// The first statement is a write so the caller's transaction takes the lock up front.
pub async fn upsert_document(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    message_id: &str,
    doc: &Document,
) -> sqlx::Result<bool> {
    let hash = content_hash(&doc.chunks);
    sqlx::query("DELETE FROM knowledge_chunks WHERE workspace_id = ?1 AND kind = ?2 AND ref_id = ?3 AND content_hash != ?4")
        .bind(workspace_id)
        .bind(doc.kind)
        .bind(&doc.ref_id)
        .bind(&hash)
        .execute(&mut *conn)
        .await?;
    let kept: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM knowledge_chunks WHERE workspace_id = ?1 AND kind = ?2 AND ref_id = ?3",
    )
    .bind(workspace_id)
    .bind(doc.kind)
    .bind(&doc.ref_id)
    .fetch_one(&mut *conn)
    .await?;
    if kept > 0 {
        return Ok(false);
    }
    let now = now_ms();
    for (position, text) in doc.chunks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO knowledge_chunks (workspace_id, kind, ref_id, message_id, position, text, embedded, created_at, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
        )
        .bind(workspace_id)
        .bind(doc.kind)
        .bind(&doc.ref_id)
        .bind(message_id)
        .bind(position as i64)
        .bind(text)
        .bind(now)
        .bind(&hash)
        .execute(&mut *conn)
        .await?;
    }
    Ok(true)
}

/// Triggers remove the FTS rows and vectors too.
pub async fn delete_document(conn: &mut SqliteConnection, workspace_id: &str, kind: &str, ref_id: &str) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM knowledge_chunks WHERE workspace_id = ?1 AND kind = ?2 AND ref_id = ?3")
        .bind(workspace_id)
        .bind(kind)
        .bind(ref_id)
        .execute(conn)
        .await?
        .rows_affected())
}

pub async fn delete_message(conn: &mut SqliteConnection, message_id: &str) -> sqlx::Result<u64> {
    Ok(sqlx::query("DELETE FROM knowledge_chunks WHERE message_id = ?1").bind(message_id).execute(conn).await?.rows_affected())
}

#[derive(sqlx::FromRow)]
struct PendingChunk {
    id: i64,
    workspace_id: String,
    kind: String,
    text: String,
}

/// Embeds this message's not-yet-embedded chunks (network call outside any transaction).
/// A chunk replaced while its embedding was in flight is left pending, never mislabelled.
pub async fn embed_pending(db: &SqlitePool, embedder: &dyn Embedder, message_id: &str) -> Result<usize, IndexError> {
    let mut done = 0;
    for _ in 0..MAX_EMBED_ROUNDS {
        let rows: Vec<PendingChunk> = sqlx::query_as(
            "SELECT id, workspace_id, kind, text FROM knowledge_chunks WHERE message_id = ?1 AND embedded = 0 ORDER BY id LIMIT ?2",
        )
        .bind(message_id)
        .bind(EMBED_ROUND)
        .fetch_all(db)
        .await?;
        if rows.is_empty() {
            break;
        }
        let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
        let vectors = embedder.embed(&texts).await?;
        if vectors.len() != rows.len() || vectors.iter().any(|v| v.len() != EMBEDDING_DIMS) {
            return Err(AiError::Permanent("embedder returned the wrong number or size of vectors".into()).into());
        }
        let mut tx = db.begin().await?;
        for (row, vector) in rows.iter().zip(&vectors) {
            let claimed = sqlx::query("UPDATE knowledge_chunks SET embedded = 1 WHERE id = ?1 AND embedded = 0 AND text = ?2")
                .bind(row.id)
                .bind(&row.text)
                .execute(&mut *tx)
                .await?
                .rows_affected()
                == 1;
            if !claimed {
                continue;
            }
            sqlx::query("DELETE FROM knowledge_vec WHERE rowid = ?1").bind(row.id).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO knowledge_vec (rowid, workspace_id, kind, embedding) VALUES (?1, ?2, ?3, ?4)")
                .bind(row.id)
                .bind(&row.workspace_id)
                .bind(&row.kind)
                .bind(to_blob(vector))
                .execute(&mut *tx)
                .await?;
            done += 1;
        }
        tx.commit().await?;
    }
    Ok(done)
}

// ---------------------------------------------------------------- search

/// Which chunk kinds a search covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Kind(&'static str),
    /// Emails and attachments (not replies): what people search.
    Documents,
}

impl Scope {
    /// Kinds are our own constants, never user input, so inlining them is safe.
    fn sql(self, column: &str) -> String {
        match self {
            Scope::All => String::new(),
            Scope::Kind(k) if [kind::EMAIL, kind::ATTACHMENT, kind::REPLY].contains(&k) => format!(" AND {column} = '{k}'"),
            Scope::Kind(_) => " AND 0".into(),
            Scope::Documents => format!(" AND {column} != '{}'", kind::REPLY),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Hit {
    pub id: i64,
    pub kind: String,
    pub ref_id: String,
    pub message_id: Option<String>,
    pub text: String,
    #[sqlx(skip)]
    pub score: f64,
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "can", "dear", "for", "from", "has", "have", "hello", "hi",
    "i", "in", "is", "it", "me", "my", "not", "of", "on", "or", "our", "please", "regards", "thanks", "that", "the",
    "this", "to", "was", "we", "will", "with", "you", "your",
];

fn terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|t| t.chars().count() >= 2 && !STOPWORDS.contains(&t.as_str()) && seen.insert(t.clone()))
        .take(MAX_FTS_TERMS)
        .collect()
}

/// FTS5 query: each term quoted (no operator injection), OR-ed so BM25 ranks by coverage.
pub fn fts_query(query: &str) -> Option<String> {
    let terms = terms(query);
    (!terms.is_empty()).then(|| terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" OR "))
}

/// Reciprocal rank fusion: score = Σ 1 / (RRF_K + rank). Ties keep first-seen order.
pub fn rrf(lists: &[Vec<i64>]) -> Vec<(i64, f64)> {
    let mut order: Vec<i64> = Vec::new();
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            let s = scores.entry(*id).or_insert_with(|| {
                order.push(*id);
                0.0
            });
            *s += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut fused: Vec<(i64, f64)> = order.into_iter().map(|id| (id, scores[&id])).collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1));
    fused
}

async fn keyword_ids(db: &SqlitePool, workspace_id: &str, fts: &str, scope: Scope, k: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(&format!(
        "SELECT c.id FROM knowledge_fts JOIN knowledge_chunks c ON c.id = knowledge_fts.rowid
         WHERE knowledge_fts MATCH ?1 AND c.workspace_id = ?2{}
         ORDER BY bm25(knowledge_fts) LIMIT ?3",
        scope.sql("c.kind")
    ))
    .bind(fts)
    .bind(workspace_id)
    .bind(k)
    .fetch_all(db)
    .await
}

async fn vector_ids(db: &SqlitePool, workspace_id: &str, vector: &[f32], scope: Scope, k: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(&format!(
        "SELECT rowid FROM knowledge_vec
         WHERE embedding MATCH ?1 AND k = ?2 AND workspace_id = ?3{}
         ORDER BY distance",
        scope.sql("kind")
    ))
    .bind(to_blob(vector))
    .bind(k)
    .bind(workspace_id)
    .fetch_all(db)
    .await
}

/// BM25 top-k ⊕ KNN top-k, fused by rank. If the query can't be embedded, falls back to
/// keywords only (logged), so search and RAG degrade instead of failing.
pub async fn hybrid_search(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    workspace_id: &str,
    query: &str,
    scope: Scope,
    k: usize,
) -> sqlx::Result<Vec<Hit>> {
    let k = k.clamp(1, 200) as i64;
    let mut lists = Vec::with_capacity(2);
    if let Some(fts) = fts_query(query) {
        lists.push(keyword_ids(db, workspace_id, &fts, scope, k).await?);
    }
    match embedder.embed(&[truncate_chars(query, MAX_QUERY_CHARS)]).await {
        Ok(v) if v.len() == 1 && v[0].len() == EMBEDDING_DIMS => lists.push(vector_ids(db, workspace_id, &v[0], scope, k).await?),
        Ok(_) => tracing::warn!("query embedding had the wrong shape; keyword search only"),
        Err(e) => tracing::warn!(error = %e, "query embedding failed; keyword search only"),
    }
    let fused = rrf(&lists);
    if fused.is_empty() {
        return Ok(vec![]);
    }
    let placeholders = vec!["?"; fused.len()].join(",");
    let sql = format!(
        "SELECT id, kind, ref_id, message_id, text FROM knowledge_chunks WHERE workspace_id = ? AND id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, Hit>(&sql).bind(workspace_id);
    for (id, _) in &fused {
        q = q.bind(id);
    }
    let mut by_id: HashMap<i64, Hit> = q.fetch_all(db).await?.into_iter().map(|h| (h.id, h)).collect();
    Ok(fused
        .into_iter()
        .filter_map(|(id, score)| by_id.remove(&id).map(|h| Hit { score, ..h }))
        .collect())
}

/// Chunk text without its title line (subject / filename).
pub fn chunk_body(text: &str) -> &str {
    text.split_once('\n').map(|(_, b)| b).unwrap_or(text)
}

/// About `max` characters around the first query term found, else the start.
pub fn snippet(text: &str, query: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let hit = terms(query).iter().filter_map(|t| {
        let t: Vec<char> = t.chars().collect();
        lower.windows(t.len()).position(|w| w == t.as_slice())
    }).min();
    let start = hit.map(|i| i.saturating_sub(max / 4)).unwrap_or(0);
    let start = if start > 0 {
        (start..chars.len()).find(|&i| chars[i - 1].is_whitespace()).unwrap_or(start)
    } else {
        0
    };
    let end = (start + max).min(chars.len());
    let mut out: String = chars[start..end].iter().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        out.insert_str(0, "… ");
    }
    if end < chars.len() {
        out.push_str(" …");
    }
    out
}

// ---------------------------------------------------------------- RAG

fn query_text(subject: &str, body: &str) -> String {
    format!("{}\n{}", subject.trim(), truncate_chars(&clean_body(body), 1_500))
}

fn in_list(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// Lets RAG skip the query-embedding call while a workspace has nothing of this kind yet.
async fn has_chunks(db: &SqlitePool, workspace_id: &str, kind: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM knowledge_chunks WHERE workspace_id = ?1 AND kind = ?2)")
        .bind(workspace_id)
        .bind(kind)
        .fetch_one(db)
        .await
}

/// Up to 5 similar past emails whose labels can be trusted: confirmed by a person,
/// set by a person, or auto-labelled with confidence ≥ 0.9 and not `other`.
/// Never fails the caller: an empty store or a retrieval error yields no examples.
pub async fn label_examples(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    workspace_id: &str,
    exclude_message_id: &str,
    subject: &str,
    body: &str,
) -> Vec<LabelExample> {
    match try_label_examples(db, embedder, workspace_id, exclude_message_id, subject, body).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "label example retrieval failed; classifying without examples");
            vec![]
        }
    }
}

async fn try_label_examples(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    workspace_id: &str,
    exclude_message_id: &str,
    subject: &str,
    body: &str,
) -> sqlx::Result<Vec<LabelExample>> {
    if !has_chunks(db, workspace_id, kind::EMAIL).await? {
        return Ok(vec![]);
    }
    let hits = hybrid_search(db, embedder, workspace_id, &query_text(subject, body), Scope::Kind(kind::EMAIL), EXAMPLE_CANDIDATES)
        .await?;
    let mut seen = HashSet::new();
    let ids: Vec<String> = hits
        .into_iter()
        .filter_map(|h| h.message_id)
        .filter(|m| m != exclude_message_id && seen.insert(m.clone()))
        .collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let sql = format!(
        "SELECT m.id, m.subject, m.body_text, c.category
         FROM email_messages m
         JOIN classifications c ON c.message_id = m.id
         JOIN labels l ON l.workspace_id = m.workspace_id AND l.key = c.category AND l.archived = 0
         WHERE m.workspace_id = ? AND m.id IN ({})
           AND (c.confirmed = 1 OR c.source = 'manual'
                OR (c.source IN ('llm', 'jev') AND c.confidence >= {TRUSTED_CONFIDENCE} AND c.category != 'other'))",
        in_list(ids.len())
    );
    let mut q = sqlx::query_as::<_, (String, String, String, String)>(&sql).bind(workspace_id);
    for id in &ids {
        q = q.bind(id);
    }
    let mut rows: HashMap<String, (String, String, String)> =
        q.fetch_all(db).await?.into_iter().map(|(id, s, b, c)| (id, (s, b, c))).collect();
    Ok(ids
        .iter()
        .filter_map(|id| rows.remove(id))
        .take(MAX_LABEL_EXAMPLES)
        .map(|(subject, body, label)| LabelExample { subject, snippet: truncate_chars(&clean_body(&body), 300), label })
        .collect())
}

/// Up to 3 replies this team approved or sent to similar emails (never for the same email).
pub async fn reply_examples(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    workspace_id: &str,
    exclude_message_id: &str,
    subject: &str,
    body: &str,
) -> Vec<ReplyExample> {
    match try_reply_examples(db, embedder, workspace_id, exclude_message_id, subject, body, None).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "reply example retrieval failed; drafting without examples");
            vec![]
        }
    }
}

/// Like `reply_examples`, but only replies to senders at `sender_domain`: drafts nobody asked
/// for must not quote what the team wrote to other organisations.
pub async fn reply_examples_from_domain(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    workspace_id: &str,
    exclude_message_id: &str,
    subject: &str,
    body: &str,
    sender_domain: &str,
) -> Vec<ReplyExample> {
    if sender_domain.is_empty() {
        return vec![];
    }
    match try_reply_examples(db, embedder, workspace_id, exclude_message_id, subject, body, Some(sender_domain)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "reply example retrieval failed; drafting without examples");
            vec![]
        }
    }
}

async fn try_reply_examples(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    workspace_id: &str,
    exclude_message_id: &str,
    subject: &str,
    body: &str,
    sender_domain: Option<&str>,
) -> sqlx::Result<Vec<ReplyExample>> {
    if !has_chunks(db, workspace_id, kind::REPLY).await? {
        return Ok(vec![]);
    }
    let hits = hybrid_search(db, embedder, workspace_id, &query_text(subject, body), Scope::Kind(kind::REPLY), EXAMPLE_CANDIDATES)
        .await?;
    let mut seen = HashSet::new();
    let ids: Vec<String> = hits
        .into_iter()
        .filter(|h| h.message_id.as_deref() != Some(exclude_message_id))
        .map(|h| h.ref_id)
        .filter(|d| seen.insert(d.clone()))
        .collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let sql = format!(
        "SELECT d.id, m.subject, d.body_text FROM drafts d JOIN email_messages m ON m.id = d.message_id
         WHERE d.workspace_id = ? AND d.status IN ('approved', 'sent')
           AND (? IS NULL OR lower(substr(m.sender_address, instr(m.sender_address, '@') + 1)) = ?)
           AND d.id IN ({})",
        in_list(ids.len())
    );
    let domain = sender_domain.map(str::to_lowercase);
    let mut q = sqlx::query_as::<_, (String, String, String)>(&sql).bind(workspace_id).bind(&domain).bind(&domain);
    for id in &ids {
        q = q.bind(id);
    }
    let mut rows: HashMap<String, (String, String)> = q.fetch_all(db).await?.into_iter().map(|(id, s, b)| (id, (s, b))).collect();
    Ok(ids
        .iter()
        .filter_map(|id| rows.remove(id))
        .take(MAX_REPLY_EXAMPLES)
        .map(|(subject, reply)| ReplyExample { subject, reply: truncate_chars(reply.trim(), 1_200) })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_are_bounded_overlapping_and_utf8_safe() {
        assert!(chunk_text("", 800, 120).is_empty());
        assert_eq!(chunk_text("short note", 800, 120), vec!["short note"]);
        let text = (0..400).map(|i| format!("wörd{i}")).collect::<Vec<_>>().join(" ");
        let chunks = chunk_text(&text, 800, 120);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 800));
        for pair in chunks.windows(2) {
            let first_word = pair[1].split(' ').next().unwrap();
            assert!(pair[0].contains(first_word), "chunks overlap on whole words");
        }
        assert!(chunks.last().unwrap().ends_with("wörd399"));
        let unbroken = "x".repeat(2_000);
        let chunks = chunk_text(&unbroken, 800, 120);
        assert_eq!(chunks.iter().map(|c| c.len()).sum::<usize>(), 2_000);
    }

    #[test]
    fn email_documents_drop_quotes_prefix_subject_and_redact() {
        let body = "Hi,\nPlease approve.\n\nMy password: hunter2\n\nOn Mon, 1 Sep, Priya wrote:\n> old stuff";
        let doc = email_document("msg_1", "Leave request", body);
        assert_eq!(doc.kind, kind::EMAIL);
        assert_eq!(doc.chunks.len(), 1);
        assert!(doc.chunks[0].starts_with("Leave request\nHi,\nPlease approve."));
        assert!(!doc.chunks[0].contains("hunter2"));
        assert!(!doc.chunks[0].contains("old stuff"));
        assert_eq!(email_document("m", "Only a subject", "").chunks, vec!["Only a subject"]);
        assert!(email_document("m", " ", " ").chunks.is_empty());
        let long = email_document("m", "S", &"word ".repeat(5_000));
        assert!(long.chunks.len() <= MAX_DOC_CHARS / (CHUNK_CHARS - CHUNK_OVERLAP) + 2);
    }

    #[test]
    fn content_hash_changes_with_content_only() {
        let a = vec!["one".to_string(), "two".to_string()];
        assert_eq!(content_hash(&a), content_hash(&a.clone()));
        assert_ne!(content_hash(&a), content_hash(&["onetwo".to_string()]));
    }

    #[test]
    fn rrf_rewards_agreement_between_lists() {
        let fused = rrf(&[vec![1, 2, 3], vec![3, 4]]);
        assert_eq!(fused[0].0, 3, "in both lists beats first place in one");
        assert_eq!(fused.iter().map(|f| f.0).collect::<Vec<_>>(), vec![3, 1, 2, 4], "ties keep first-seen order");
        assert!((fused[0].1 - (1.0 / 63.0 + 1.0 / 61.0)).abs() < 1e-12);
        assert!(rrf(&[]).is_empty());
        assert_eq!(rrf(&[vec![7]])[0], (7, 1.0 / 61.0));
    }

    #[test]
    fn fts_query_quotes_terms_and_drops_noise() {
        assert_eq!(fts_query("Java  spring\"); DROP pune").unwrap(), "\"java\" OR \"spring\" OR \"drop\" OR \"pune\"");
        assert_eq!(fts_query("the and to").as_deref(), None);
        assert_eq!(fts_query("NEAR(a b) OR c*").unwrap(), "\"near\"");
    }

    #[test]
    fn snippets_center_on_the_match() {
        let text = format!("{} timesheet for September {}", "intro ".repeat(60), "tail ".repeat(60));
        let s = snippet(&text, "september", 120);
        assert!(s.contains("September") && s.starts_with("… ") && s.ends_with(" …"));
        assert!(s.chars().count() <= 126);
        assert_eq!(snippet("short text", "zzz", 120), "short text");
        assert_eq!(chunk_body("Subject\nBody here"), "Body here");
    }
}
