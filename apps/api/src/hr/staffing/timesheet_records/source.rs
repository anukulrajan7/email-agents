use super::*;

pub(super) async fn load_people(db: &SqlitePool, ws: &str) -> sqlx::Result<(Vec<Candidate>, Aliases)> {
    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, employee_code FROM people WHERE workspace_id = ?1 AND active = 1").bind(ws).fetch_all(db).await?;
    let emails: Vec<(String, String)> = sqlx::query_as("SELECT person_id, email FROM person_emails WHERE workspace_id = ?1").bind(ws).fetch_all(db).await?;
    let mut by_person: HashMap<String, Vec<String>> = HashMap::new();
    for (p, e) in emails {
        by_person.entry(p).or_default().push(e);
    }
    let people = rows.into_iter().map(|(id, name, code)| Candidate { emails: by_person.remove(&id).unwrap_or_default(), id, name, code }).collect();
    let mut aliases = Aliases::default();
    let rows: Vec<(String, String, String)> = sqlx::query_as("SELECT kind, value, person_id FROM timesheet_aliases WHERE workspace_id = ?1").bind(ws).fetch_all(db).await?;
    for (kind, value, person_id) in rows {
        if kind == "code" { aliases.codes.insert(value, person_id) } else { aliases.names.insert(value, person_id) };
    }
    Ok((people, aliases))
}

/// `Some(true)` for a forwarded-message header, `Some(false)` for the header of a quoted reply.
fn quote_marker(lines: &[&str], i: usize) -> Option<bool> {
    let lower = lines[i].trim().to_lowercase();
    if lower.starts_with("begin forwarded message") || (lower.starts_with("--") && lower.contains("forwarded message")) {
        return Some(true);
    }
    if lower.starts_with("--") && lower.contains("original message") {
        return Some(false);
    }
    let wraps = lines.get(i + 1).is_some_and(|n| n.trim().to_lowercase().starts_with("wrote:"));
    (lower.starts_with("on ") && (lower.ends_with("wrote:") || wraps)).then_some(false)
}

/// What the sender wrote in this email: quoted lines (`>`) and everything under an "On ... wrote:" or "Original
/// message" header are an earlier email. A forward whose own words are only a short note is the exception: the
/// forwarded sheet is what the email is about.
fn own_text(subject: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let fresh = |part: &[&str]| -> Vec<String> { part.iter().filter(|l| !l.trim_start().starts_with('>')).map(|l| (*l).to_string()).collect() };
    let Some((cut, marker_forward)) = (0..lines.len()).find_map(|i| quote_marker(&lines, i).map(|forward| (i, forward))) else {
        return fresh(&lines).join("\n");
    };
    let mut kept = fresh(&lines[..cut]);
    let note_chars: usize = kept.iter().map(|l| l.trim().chars().count()).sum();
    let note_lines = kept.iter().filter(|l| !l.trim().is_empty()).count();
    let subject = subject.trim().to_lowercase();
    let forward = marker_forward || subject.starts_with("fwd:") || subject.starts_with("fw:");
    if forward && note_chars <= 300 && note_lines <= 6 {
        kept.extend(lines[cut + 1..].iter().map(|l| l.trim_start_matches(['>', ' ']).to_string()));
    }
    kept.join("\n")
}

pub(super) async fn load_units(db: &SqlitePool, message_id: &str, msg: &MessageRow) -> sqlx::Result<(Vec<Unit>, bool)> {
    let rows: Vec<AttachmentRow> = sqlx::query_as(
        "SELECT id, filename, text_content, extract_status, extract_kind FROM attachments WHERE message_id = ?1 ORDER BY filename",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?;
    let mut units = vec![Unit {
        source_key: format!("msg:{message_id}:body"),
        source: "pasted",
        attachment_id: None,
        filename: None,
        kind: None,
        text: own_text(&msg.subject, &msg.body_text),
        unreadable: false,
    }];
    let mut waiting = false;
    let sole = rows.len() == 1;
    for a in rows {
        let text = a.text_content.as_deref().unwrap_or_default().trim().to_string();
        if !text.is_empty() {
            units.push(Unit { source_key: format!("msg:{message_id}:att:{}", a.id), source: "attachment", attachment_id: Some(a.id), filename: Some(a.filename), kind: a.extract_kind, text, unreadable: false });
        } else if a.extract_status.is_none() {
            waiting = true;
        } else if NAME_CUE.is_match(&a.filename) || sole {
            units.push(Unit { source_key: format!("msg:{message_id}:att:{}", a.id), source: "attachment", attachment_id: Some(a.id), filename: Some(a.filename), kind: a.extract_kind, text: String::new(), unreadable: true });
        }
    }
    Ok((units, waiting))
}

pub(super) fn kind_label(kind: Option<&str>, filename: Option<&str>) -> Option<String> {
    let ext = filename.and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_lowercase());
    let label = match (kind, ext.as_deref()) {
        (_, Some("xlsx" | "xls" | "xlsm" | "xlsb" | "ods")) | (Some("sheet" | "spreadsheet"), _) => "Excel",
        (_, Some("csv" | "tsv")) => "CSV",
        (_, Some("pdf")) | (Some("pdf"), _) => "PDF",
        (_, Some("docx" | "doc" | "rtf" | "odt")) | (Some("docx" | "word"), _) => "Word",
        (Some("image"), _) | (_, Some("png" | "jpg" | "jpeg" | "webp")) => "Image",
        _ => return filename.map(|_| "File".to_string()),
    };
    Some(label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_and_forwards_keep_only_what_the_sender_wrote() {
        let table = "| Date | Hours |\n| 2026-09-01 | 8 |";
        assert_eq!(own_text("Re: timesheet", &format!("Thanks!\n\nOn Mon, 5 Oct 2026 at 10:00, Ravi <r@x.test> wrote:\n> {table}")).trim(), "Thanks!");
        assert_eq!(own_text("Re: x", "Looks right.\n> | 2026-09-01 | 8 |").trim(), "Looks right.");
        let fwd = own_text("Fwd: Sept timesheet", &format!("FYI\n\n---------- Forwarded message ---------\nFrom: Ravi\n\n{table}"));
        assert!(fwd.contains("FYI") && fwd.contains("2026-09-01 | 8"));
        let long_note = format!("{}\n\n---------- Forwarded message ---------\n{table}", "Please read this carefully. ".repeat(20));
        assert!(!own_text("Fwd: x", &long_note).contains("2026-09-01"));
        let wrapped = own_text("Re: x", "ok\nOn Mon, 5 Oct 2026 at 10:00, Ravi Kumar <ravi@x.test>\nwrote:\n2026-09-01 | 8");
        assert_eq!(wrapped.trim(), "ok");
    }
}
