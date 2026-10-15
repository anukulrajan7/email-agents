//! Referral ↔ requirement matching: typed fields extracted from emails (and CV / JD
//! attachments), deterministic scoring with human-readable reasons, attachment text.

use std::{collections::BTreeSet, io::Read, sync::OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- extracted fields

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RequirementFields {
    pub client: Option<String>,
    pub role_title: String,
    pub must_have_skills: Vec<String>,
    pub nice_to_have_skills: Vec<String>,
    pub min_experience: Option<f64>,
    pub max_experience: Option<f64>,
    pub location: Option<String>,
    pub positions: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CandidateFields {
    pub name: String,
    pub email: Option<String>,
    pub skills: Vec<String>,
    pub total_experience: Option<f64>,
    pub location: Option<String>,
    pub notice_days: Option<i64>,
    pub referred_by: Option<String>,
    pub summary: Option<String>,
}

// ---------------------------------------------------------------- normalisation

pub use crate::replies::skills::{canonical_skill, canonical_skills};
use crate::replies::skills::skills_in;

const CITIES: &[(&str, &[&str])] = &[
    ("Bengaluru", &["bengaluru", "bangalore"]),
    ("Pune", &["pune"]),
    ("Hyderabad", &["hyderabad"]),
    ("Chennai", &["chennai"]),
    ("Mumbai", &["mumbai", "navi mumbai"]),
    ("Delhi NCR", &["delhi", "noida", "gurgaon", "gurugram", "ncr"]),
    ("Kolkata", &["kolkata"]),
    ("Remote", &["remote", "work from home", "wfh"]),
];

pub fn canonical_city(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    CITIES
        .iter()
        .find(|(_, aliases)| aliases.iter().any(|a| s.contains(a)))
        .map(|(c, _)| c.to_string())
        .unwrap_or_else(|| raw.trim().to_string())
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    crate::replies::skills::find_word(haystack, needle).is_some()
}

fn city_in(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    CITIES.iter().find(|(_, a)| a.iter().any(|x| contains_word(&lower, x))).map(|(c, _)| c.to_string())
}

fn re(pattern: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

fn experience_range(text: &str) -> (Option<f64>, Option<f64>) {
    static RANGE: OnceLock<Regex> = OnceLock::new();
    static SINGLE: OnceLock<Regex> = OnceLock::new();
    let lower = text.to_lowercase();
    if let Some(c) = re(r"(\d+(?:\.\d+)?)\s*(?:-|to|–)\s*(\d+(?:\.\d+)?)\s*\+?\s*(?:years|yrs|yr|year)", &RANGE).captures(&lower) {
        return (c[1].parse().ok(), c[2].parse().ok());
    }
    if let Some(c) = re(r"(\d+(?:\.\d+)?)\s*\+?\s*(?:years|yrs|yr|year)", &SINGLE).captures(&lower) {
        let v: Option<f64> = c[1].parse().ok();
        return (v, None);
    }
    (None, None)
}

// ---------------------------------------------------------------- rules extraction (no-AI fallback)

pub fn rules_requirement(subject: &str, text: &str, sender_domain: Option<&str>) -> RequirementFields {
    static POSITIONS: OnceLock<Regex> = OnceLock::new();
    let all = format!("{subject}\n{text}");
    let (min_experience, max_experience) = experience_range(&all);
    let role_title = subject
        .trim()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .split_once(':')
        .map(|(head, tail)| if head.to_lowercase().contains("requirement") || head.to_lowercase().starts_with("fwd") { tail } else { subject })
        .unwrap_or(subject)
        .trim()
        .to_string();
    RequirementFields {
        client: sender_domain.map(|d| title_case(d.split('.').next().unwrap_or(d))).filter(|d| !d.is_empty()),
        role_title: if role_title.is_empty() { "Untitled requirement".into() } else { role_title },
        must_have_skills: skills_in(&all),
        nice_to_have_skills: vec![],
        min_experience,
        max_experience,
        location: city_in(&all),
        positions: re(r"(?i)(\d+)\s+(?:positions|openings|developers|engineers|resources|profiles)", &POSITIONS)
            .captures(&all)
            .and_then(|c| c[1].parse().ok()),
    }
}

/// "capgemini" → "Capgemini" (domain-derived client names).
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

pub fn rules_candidate(subject: &str, text: &str, sender: &str) -> CandidateFields {
    static NAME: OnceLock<Regex> = OnceLock::new();
    static NOTICE: OnceLock<Regex> = OnceLock::new();
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    let all = format!("{subject}\n{text}");
    let name = re(r"(?im)^\s*(?:candidate\s+)?name\s*[:\-]\s*([A-Za-z][A-Za-z .']{1,60})$", &NAME)
        .captures(&all)
        .map(|c| c[1].trim().to_string())
        .or_else(|| {
            subject
                .split_once(['-', ':', '–'])
                .map(|(_, rest)| rest.trim().to_string())
                .filter(|r| !r.is_empty() && r.split_whitespace().count() <= 4)
        })
        .unwrap_or_else(|| "Unnamed candidate".into());
    let lower = all.to_lowercase();
    let notice_days = if lower.contains("immediate joiner") || lower.contains("immediately available") {
        Some(0)
    } else {
        re(r"(?i)notice(?:\s+period)?\s*[:\-]?\s*(\d+)\s*days", &NOTICE).captures(&all).and_then(|c| c[1].parse().ok())
    };
    CandidateFields {
        name,
        email: re(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}", &EMAIL)
            .find_iter(text)
            .map(|m| m.as_str().to_lowercase())
            .find(|e| e != sender),
        skills: skills_in(&all),
        total_experience: experience_range(&all).0,
        location: city_in(&all),
        notice_days,
        referred_by: Some(sender.to_string()).filter(|s| !s.is_empty()),
        summary: None,
    }
}

// ---------------------------------------------------------------- scoring

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reason {
    pub ok: bool,
    pub text: String,
}

/// Points per component; they add up to 100.
pub const SKILL_WEIGHT: f64 = 50.0;
pub const EXPERIENCE_WEIGHT: f64 = 20.0;
pub const LOCATION_WEIGHT: f64 = 15.0;
pub const SEMANTIC_WEIGHT: f64 = 15.0;
/// Cosine similarity (profile embeddings) at or below which semantic fit earns nothing, and at or above which
/// it earns full points; linear in between.
pub const SIMILARITY_FLOOR: f32 = 0.40;
pub const SIMILARITY_FULL: f32 = 0.80;
/// Reason wording thresholds.
pub const SIMILARITY_CLOSE: f32 = 0.65;
pub const SIMILARITY_SOME: f32 = 0.50;

/// Semantic-fit points for a cosine similarity; `None` (no embedding available) earns half, like other unknowns.
pub fn semantic_points(similarity: Option<f32>) -> f64 {
    match similarity {
        Some(s) => SEMANTIC_WEIGHT * ((s - SIMILARITY_FLOOR) / (SIMILARITY_FULL - SIMILARITY_FLOOR)).clamp(0.0, 1.0) as f64,
        None => SEMANTIC_WEIGHT / 2.0,
    }
}

/// 0–100: must-have skills (50, related skills count half), experience fit (20), location (15) and semantic fit
/// (15, cosine similarity of the two profile embeddings). Every point comes with a reason a recruiter can check.
pub fn score(req: &RequirementFields, cand: &CandidateFields, similarity: Option<f32>) -> (u8, Vec<Reason>) {
    let mut reasons = Vec::new();
    let must = canonical_skills(&req.must_have_skills);
    let have: BTreeSet<String> = canonical_skills(&cand.skills).into_iter().collect();
    let skill_points = if must.is_empty() {
        reasons.push(Reason { ok: true, text: "Requirement lists no specific skills".into() });
        SKILL_WEIGHT / 2.0
    } else {
        let matched: Vec<&String> = must.iter().filter(|s| have.contains(*s)).collect();
        let mut related: Vec<(&String, &str)> = vec![];
        let mut missing: Vec<&String> = vec![];
        for s in must.iter().filter(|s| !have.contains(*s)) {
            match crate::replies::skills::related(s).iter().find(|r| have.contains(**r)) {
                Some(r) => related.push((s, r)),
                None => missing.push(s),
            }
        }
        if !matched.is_empty() {
            reasons.push(Reason {
                ok: true,
                text: format!("Has {}/{} must-have skills: {}", matched.len(), must.len(), join(&matched)),
            });
        }
        for (wanted, r) in &related {
            reasons.push(Reason { ok: true, text: format!("Has related skill: {r} (for {wanted})") });
        }
        if !missing.is_empty() {
            reasons.push(Reason { ok: false, text: format!("Missing: {}", join(&missing)) });
        }
        SKILL_WEIGHT * (matched.len() as f64 + 0.5 * related.len() as f64) / must.len() as f64
    };

    let exp_points = match (cand.total_experience, req.min_experience, req.max_experience) {
        (Some(y), Some(min), max) if y >= min && max.is_none_or(|m| y <= m) => {
            reasons.push(Reason { ok: true, text: format!("{y} yrs fits {}", band(min, max)) });
            EXPERIENCE_WEIGHT
        }
        (Some(y), Some(min), _) if y < min => {
            let gap = min - y;
            reasons.push(Reason { ok: false, text: format!("{y} yrs is below {}", band(min, req.max_experience)) });
            if gap <= 1.0 { EXPERIENCE_WEIGHT / 2.0 } else { 0.0 }
        }
        (Some(y), Some(min), Some(max)) => {
            reasons.push(Reason { ok: false, text: format!("{y} yrs is above {}", band(min, Some(max))) });
            EXPERIENCE_WEIGHT / 2.0
        }
        (None, Some(_), _) => {
            reasons.push(Reason { ok: false, text: "Candidate's experience unknown".into() });
            6.0
        }
        _ => 12.0,
    };

    let loc_points = match (req.location.as_deref(), cand.location.as_deref()) {
        (Some(r), Some(c)) if canonical_city(r) == canonical_city(c) || canonical_city(r) == "Remote" => {
            reasons.push(Reason { ok: true, text: format!("Location {} matches", canonical_city(c)) });
            LOCATION_WEIGHT
        }
        (Some(r), Some(c)) => {
            reasons.push(Reason { ok: false, text: format!("Based in {}, role is in {}", canonical_city(c), canonical_city(r)) });
            3.0
        }
        _ => 8.0,
    };

    if let Some(s) = similarity {
        let (ok, text) = if s >= SIMILARITY_CLOSE {
            (true, format!("Profile closely matches the role (similarity {s:.2})"))
        } else if s >= SIMILARITY_SOME {
            (true, format!("Profile is somewhat similar to the role (similarity {s:.2})"))
        } else {
            (false, format!("Profile is not much like the role (similarity {s:.2})"))
        };
        reasons.push(Reason { ok, text });
    }
    let sem_points = semantic_points(similarity);

    if let Some(days) = cand.notice_days {
        reasons.push(Reason {
            ok: days <= 30,
            text: if days == 0 { "Immediate joiner".into() } else { format!("Notice period {days} days") },
        });
    }
    let total = (skill_points + exp_points + loc_points + sem_points).round().clamp(0.0, 100.0) as u8;
    (total, reasons)
}

fn join(items: &[&String]) -> String {
    items.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

fn band(min: f64, max: Option<f64>) -> String {
    match max {
        Some(m) => format!("{min}–{m} yrs"),
        None => format!("{min}+ yrs"),
    }
}

/// Matches below this are not worth a recruiter's attention.
pub const MIN_MATCH_SCORE: u8 = 40;
/// Suggested matches at or above this go in the daily match digest.
pub const STRONG_MATCH_SCORE: u8 = 70;

// ---------------------------------------------------------------- profiles (semantic recall)

/// Characters of source email / attachment text appended to a profile.
pub const PROFILE_SOURCE_CHARS: usize = 1_500;

fn experience_line(min: Option<f64>, max: Option<f64>) -> Option<String> {
    match (min, max) {
        (Some(a), Some(b)) => Some(format!("{a}–{b} years experience")),
        (Some(a), None) => Some(format!("{a}+ years experience")),
        (None, Some(b)) => Some(format!("up to {b} years experience")),
        (None, None) => None,
    }
}

fn profile(lines: Vec<Option<String>>, source: &str) -> String {
    let mut out: Vec<String> = lines.into_iter().flatten().filter(|l| !l.trim().is_empty()).collect();
    let source = crate::mail::sources::normalize::truncate_chars(source.trim(), PROFILE_SOURCE_CHARS);
    if !source.is_empty() {
        out.push(source);
    }
    out.join("\n")
}

/// What gets embedded for a requirement: role, skills, experience, location, then the JD text.
pub fn requirement_profile(r: &RequirementFields, source: &str) -> String {
    let skills = canonical_skills(&[r.must_have_skills.clone(), r.nice_to_have_skills.clone()].concat());
    profile(
        vec![
            Some(r.role_title.trim().to_string()),
            (!skills.is_empty()).then(|| format!("Skills: {}", skills.join(", "))),
            experience_line(r.min_experience, r.max_experience),
            r.location.as_deref().map(|l| format!("Location: {}", canonical_city(l))),
        ],
        source,
    )
}

/// What gets embedded for a candidate: summary, skills, experience, location, then the referral / CV text.
pub fn candidate_profile(c: &CandidateFields, source: &str) -> String {
    let skills = canonical_skills(&c.skills);
    profile(
        vec![
            c.summary.clone(),
            (!skills.is_empty()).then(|| format!("Skills: {}", skills.join(", "))),
            c.total_experience.map(|y| format!("{y} years experience")),
            c.location.as_deref().map(|l| format!("Location: {}", canonical_city(l))),
        ],
        source,
    )
}

// ---------------------------------------------------------------- attachment text

pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 30_000;

pub enum TextResult {
    Text(String),
    Unsupported,
    Failed(String),
}

/// Text from CVs / job descriptions. PDF and DOCX only; nothing is executed.
pub fn attachment_text(filename: &str, media_type: Option<&str>, bytes: &[u8]) -> TextResult {
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return TextResult::Unsupported;
    }
    let lower = filename.to_lowercase();
    let mt = media_type.unwrap_or_default();
    let text = if lower.ends_with(".pdf") || mt == "application/pdf" {
        pdf_extract::extract_text_from_mem(bytes).map_err(|e| e.to_string())
    } else if lower.ends_with(".docx") || mt.contains("wordprocessingml") {
        docx_text(bytes)
    } else if lower.ends_with(".txt") || mt.starts_with("text/plain") {
        Ok(String::from_utf8_lossy(bytes).into_owned())
    } else {
        return crate::mail::attachments::text_result(filename, media_type, bytes);
    };
    match text {
        Ok(t) => TextResult::Text(crate::mail::sources::normalize::truncate_chars(t.trim(), MAX_TEXT_CHARS)),
        Err(e) => TextResult::Failed(e),
    }
}

/// Parsers get this long before the job records a failure.
pub const PARSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Zip-bomb guards for DOCX: entries, declared uncompressed total, and the document part.
const MAX_DOCX_ENTRIES: usize = 2_000;
const MAX_DOCX_UNCOMPRESSED: u64 = 100 * 1024 * 1024;
const MAX_DOCX_XML: u64 = 20 * 1024 * 1024;

/// `attachment_text` off the async runtime, isolated: a parser panic or a parse that runs past
/// `PARSE_TIMEOUT` becomes `TextResult::Failed`, never a dead worker. (A timed-out parse keeps
/// its blocking thread until it ends; it can't hold up the job.)
pub async fn attachment_text_isolated(filename: String, media_type: Option<String>, bytes: Vec<u8>) -> TextResult {
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return TextResult::Unsupported;
    }
    let parse = tokio::task::spawn_blocking(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| attachment_text(&filename, media_type.as_deref(), &bytes)))
    });
    match tokio::time::timeout(PARSE_TIMEOUT, parse).await {
        Ok(Ok(Ok(result))) => result,
        Ok(Ok(Err(_))) | Ok(Err(_)) => TextResult::Failed("the attachment parser crashed on this file".into()),
        Err(_) => TextResult::Failed(format!("reading the attachment took longer than {}s", PARSE_TIMEOUT.as_secs())),
    }
}

fn docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() > MAX_DOCX_ENTRIES {
        return Err("DOCX has too many parts".into());
    }
    let mut declared = 0u64;
    for i in 0..archive.len() {
        declared = declared.saturating_add(archive.by_index_raw(i).map_err(|e| e.to_string())?.size());
        if declared > MAX_DOCX_UNCOMPRESSED {
            return Err("DOCX expands to more than 100 MB".into());
        }
    }
    let mut xml = String::new();
    let document = archive.by_name("word/document.xml").map_err(|e| e.to_string())?;
    if document.size() > MAX_DOCX_XML {
        return Err("DOCX document part is too large".into());
    }
    document.take(MAX_DOCX_XML).read_to_string(&mut xml).map_err(|e| e.to_string())?;
    let with_breaks = xml.replace("</w:p>", "\n").replace("<w:tab/>", "\t").replace("<w:br/>", "\n");
    Ok(crate::mail::sources::normalize::html_to_text(&with_breaks))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docx(entries: usize, document: &str) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for i in 0..entries {
            zip.start_file(format!("word/media/{i}.xml"), opts).unwrap();
        }
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn attachment_parsing_is_bounded_and_isolated() {
        let ok = docx(0, "<w:p>Java developer</w:p>");
        assert!(matches!(attachment_text_isolated("cv.docx".into(), None, ok).await, TextResult::Text(t) if t.contains("Java developer")));
        let bomb = docx(MAX_DOCX_ENTRIES + 1, "<w:p>x</w:p>");
        assert!(matches!(attachment_text_isolated("cv.docx".into(), None, bomb).await, TextResult::Failed(e) if e.contains("too many")));
        let garbage = b"%PDF-1.7\n\x00\x01 not really a pdf".to_vec();
        assert!(matches!(attachment_text_isolated("cv.pdf".into(), None, garbage).await, TextResult::Failed(_)));
        let huge = vec![b'a'; MAX_ATTACHMENT_BYTES + 1];
        assert!(matches!(attachment_text_isolated("cv.txt".into(), None, huge).await, TextResult::Unsupported));
        let long = "word ".repeat(MAX_TEXT_CHARS);
        assert!(matches!(attachment_text_isolated("cv.txt".into(), None, long.into_bytes()).await,
            TextResult::Text(t) if t.chars().count() <= MAX_TEXT_CHARS));
    }

    #[test]
    fn rules_extract_requirement_and_candidate() {
        let r = rules_requirement(
            "Requirement: Java Developer",
            "Need 3 developers with Java, Spring Boot and Microservices. 5-8 yrs. Location Pune.",
            Some("capgemini.com"),
        );
        assert_eq!(r.role_title, "Java Developer");
        assert_eq!(r.client.as_deref(), Some("Capgemini"));
        assert_eq!(r.must_have_skills, ["java", "spring boot", "microservices"]);
        assert_eq!((r.min_experience, r.max_experience, r.positions), (Some(5.0), Some(8.0), Some(3)));
        assert_eq!(r.location.as_deref(), Some("Pune"));

        let c = rules_candidate(
            "Referral - Rahul Verma",
            "Hi, referring my friend.\nName: Rahul Verma\n6 years in Java, SpringBoot, AWS.\nBangalore. Notice period 30 days. rahul@mail.com",
            "friend@acme.com",
        );
        assert_eq!(c.name, "Rahul Verma");
        assert_eq!(c.skills, ["java", "spring boot", "aws"]);
        assert_eq!((c.total_experience, c.notice_days), (Some(6.0), Some(30)));
        assert_eq!(c.location.as_deref(), Some("Bengaluru"));
        assert_eq!(c.email.as_deref(), Some("rahul@mail.com"));
    }

    #[test]
    fn scoring_is_explainable() {
        let req = RequirementFields {
            role_title: "Java Dev".into(),
            must_have_skills: vec!["Java".into(), "Spring Boot".into(), "Kafka".into()],
            min_experience: Some(5.0),
            max_experience: Some(8.0),
            location: Some("Pune".into()),
            ..Default::default()
        };
        let good = CandidateFields {
            name: "A".into(),
            skills: vec!["java".into(), "springboot".into()],
            total_experience: Some(6.0),
            location: Some("pune".into()),
            notice_days: Some(15),
            ..Default::default()
        };
        let (s, reasons) = score(&req, &good, Some(0.8));
        assert_eq!(s, 83); // 33.3 skills + 20 exp + 15 loc + 15 semantic
        assert!(reasons.iter().any(|r| r.ok && r.text.starts_with("Has 2/3")));
        assert!(reasons.iter().any(|r| !r.ok && r.text == "Missing: kafka"));
        assert!(reasons.iter().any(|r| r.ok && r.text == "Profile closely matches the role (similarity 0.80)"));
        assert_eq!(score(&req, &good, None).0, 76, "no embedding: half the semantic points, no reason");
        assert!(!score(&req, &good, None).1.iter().any(|r| r.text.contains("similarity")));

        let weak = CandidateFields { skills: vec!["python".into()], total_experience: Some(2.0), location: Some("Chennai".into()), ..good };
        assert!(score(&req, &weak, Some(1.0)).0 < MIN_MATCH_SCORE);
    }

    #[test]
    fn synonyms_match_and_related_skills_earn_half() {
        let req = RequirementFields {
            role_title: "Full-stack developer".into(),
            must_have_skills: vec!["React".into(), "Spring Boot".into()],
            ..Default::default()
        };
        let synonyms = CandidateFields { skills: vec!["ReactJS".into(), "SpringBoot".into()], ..Default::default() };
        let (s, reasons) = score(&req, &synonyms, Some(0.4));
        assert_eq!(s, 70); // 50 skills + 12 exp (not specified) + 8 loc (unknown) + 0 semantic
        assert!(reasons.iter().any(|r| r.ok && r.text == "Has 2/2 must-have skills: react, spring boot"), "{reasons:?}");
        assert!(reasons.iter().any(|r| !r.ok && r.text.starts_with("Profile is not much like")));

        let related = CandidateFields { skills: vec!["Next.js".into(), "Spring".into()], ..Default::default() };
        let (s, reasons) = score(&req, &related, Some(0.4));
        assert_eq!(s, 45); // 2 × half credit = 25 skills + 12 + 8
        assert!(reasons.iter().any(|r| r.ok && r.text == "Has related skill: next.js (for react)"), "{reasons:?}");
        assert!(reasons.iter().any(|r| r.ok && r.text == "Has related skill: spring (for spring boot)"), "{reasons:?}");
        assert!(!reasons.iter().any(|r| r.text.starts_with("Missing")));
    }

    #[test]
    fn semantic_points_are_linear_between_floor_and_full() {
        assert_eq!(semantic_points(Some(0.2)), 0.0);
        assert_eq!(semantic_points(Some(0.95)), SEMANTIC_WEIGHT);
        assert!((semantic_points(Some(0.6)) - SEMANTIC_WEIGHT / 2.0).abs() < 1e-6);
        assert_eq!(semantic_points(None), SEMANTIC_WEIGHT / 2.0);
        assert_eq!(SKILL_WEIGHT + EXPERIENCE_WEIGHT + LOCATION_WEIGHT + SEMANTIC_WEIGHT, 100.0);
    }

    #[test]
    fn profiles_lead_with_structured_fields() {
        let r = RequirementFields {
            role_title: "Java Developer".into(),
            must_have_skills: vec!["SpringBoot".into(), "java".into()],
            min_experience: Some(5.0),
            max_experience: Some(8.0),
            location: Some("bangalore".into()),
            ..Default::default()
        };
        assert_eq!(
            requirement_profile(&r, "  Need Java devs  "),
            "Java Developer\nSkills: java, spring boot\n5–8 years experience\nLocation: Bengaluru\nNeed Java devs"
        );
        let c = CandidateFields { skills: vec!["k8s".into()], total_experience: Some(4.0), ..Default::default() };
        assert_eq!(candidate_profile(&c, ""), "Skills: kubernetes\n4 years experience");
        assert!(requirement_profile(&r, &"x".repeat(5_000)).chars().count() < PROFILE_SOURCE_CHARS + 200);
    }

    #[test]
    fn word_boundaries_and_canonical_names() {
        assert!(contains_word("knows .net and c#", ".net"));
        assert!(!contains_word("javascript", "java"));
        assert_eq!(canonical_skill("K8s"), "kubernetes");
        assert_eq!(canonical_city("Gurugram, Haryana"), "Delhi NCR");
    }
}
