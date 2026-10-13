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

/// Skills we recognise in free text (rules fallback) and canonical names for aliases.
const SKILLS: &[(&str, &[&str])] = &[
    ("java", &["java", "core java", "j2ee"]),
    ("spring boot", &["spring boot", "springboot", "spring"]),
    ("microservices", &["microservices", "micro services"]),
    ("python", &["python", "django", "flask"]),
    ("javascript", &["javascript", "js", "es6"]),
    ("typescript", &["typescript"]),
    ("react", &["react", "reactjs", "react.js"]),
    ("angular", &["angular", "angularjs"]),
    ("node", &["node", "nodejs", "node.js"]),
    (".net", &[".net", "dotnet", "c#", "asp.net"]),
    ("sql", &["sql", "mysql", "postgres", "postgresql", "oracle", "pl/sql"]),
    ("aws", &["aws", "amazon web services"]),
    ("azure", &["azure"]),
    ("gcp", &["gcp", "google cloud"]),
    ("devops", &["devops", "ci/cd", "jenkins"]),
    ("kubernetes", &["kubernetes", "k8s"]),
    ("docker", &["docker"]),
    ("salesforce", &["salesforce", "sfdc"]),
    ("sap", &["sap", "sap abap", "sap fico", "sap mm"]),
    ("testing", &["testing", "qa", "manual testing"]),
    ("selenium", &["selenium", "automation testing"]),
    ("data engineering", &["data engineer", "data engineering", "etl", "spark", "pyspark"]),
    ("power bi", &["power bi", "powerbi", "tableau"]),
    ("golang", &["golang", "go lang"]),
    ("rust", &["rust"]),
    ("android", &["android", "kotlin"]),
    ("ios", &["ios", "swift"]),
    ("flutter", &["flutter", "dart"]),
    ("machine learning", &["machine learning", "ml", "deep learning", "ai/ml"]),
];

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

/// Lowercase canonical skill name ("ReactJS" → "react", "k8s" → "kubernetes").
pub fn canonical_skill(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    SKILLS
        .iter()
        .find(|(_, aliases)| aliases.contains(&s.as_str()))
        .map(|(canon, _)| canon.to_string())
        .unwrap_or(s)
}

pub fn canonical_skills(list: &[String]) -> Vec<String> {
    let set: BTreeSet<String> = list.iter().map(|s| canonical_skill(s)).filter(|s| !s.is_empty()).collect();
    set.into_iter().collect()
}

pub fn canonical_city(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    CITIES
        .iter()
        .find(|(_, aliases)| aliases.iter().any(|a| s.contains(a)))
        .map(|(c, _)| c.to_string())
        .unwrap_or_else(|| raw.trim().to_string())
}

fn contains_word(haystack: &str, needle: &str) -> bool {
    // Word-boundary match that also works for tokens like ".net" or "c#".
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let i = start + pos;
        let before = haystack[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = haystack[i + needle.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        start = i + needle.len();
    }
    false
}

fn skills_in(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    SKILLS
        .iter()
        .filter(|(_, aliases)| aliases.iter().any(|a| contains_word(&lower, a)))
        .map(|(c, _)| c.to_string())
        .collect()
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
        client: sender_domain.map(|d| d.split('.').next().unwrap_or(d).to_string()).filter(|d| !d.is_empty()),
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

/// 0–100: must-have skill coverage (60), experience fit (25), location (15).
/// Every point comes with a reason a recruiter can check.
pub fn score(req: &RequirementFields, cand: &CandidateFields) -> (u8, Vec<Reason>) {
    let mut reasons = Vec::new();
    let must = canonical_skills(&req.must_have_skills);
    let have: BTreeSet<String> = canonical_skills(&cand.skills).into_iter().collect();
    let skill_points = if must.is_empty() {
        reasons.push(Reason { ok: true, text: "Requirement lists no specific skills".into() });
        30.0
    } else {
        let matched: Vec<&String> = must.iter().filter(|s| have.contains(*s)).collect();
        let missing: Vec<&String> = must.iter().filter(|s| !have.contains(*s)).collect();
        if !matched.is_empty() {
            reasons.push(Reason {
                ok: true,
                text: format!("Has {}/{} must-have skills: {}", matched.len(), must.len(), join(&matched)),
            });
        }
        if !missing.is_empty() {
            reasons.push(Reason { ok: false, text: format!("Missing: {}", join(&missing)) });
        }
        60.0 * matched.len() as f64 / must.len() as f64
    };

    let exp_points = match (cand.total_experience, req.min_experience, req.max_experience) {
        (Some(y), Some(min), max) if y >= min && max.is_none_or(|m| y <= m) => {
            reasons.push(Reason { ok: true, text: format!("{y} yrs fits {}", band(min, max)) });
            25.0
        }
        (Some(y), Some(min), _) if y < min => {
            let gap = min - y;
            reasons.push(Reason { ok: false, text: format!("{y} yrs is below {}", band(min, req.max_experience)) });
            if gap <= 1.0 { 12.0 } else { 0.0 }
        }
        (Some(y), Some(min), Some(max)) => {
            reasons.push(Reason { ok: false, text: format!("{y} yrs is above {}", band(min, Some(max))) });
            12.0
        }
        (None, Some(_), _) => {
            reasons.push(Reason { ok: false, text: "Candidate's experience unknown".into() });
            8.0
        }
        _ => 15.0,
    };

    let loc_points = match (req.location.as_deref(), cand.location.as_deref()) {
        (Some(r), Some(c)) if canonical_city(r) == canonical_city(c) || canonical_city(r) == "Remote" => {
            reasons.push(Reason { ok: true, text: format!("Location {} ✓", canonical_city(c)) });
            15.0
        }
        (Some(r), Some(c)) => {
            reasons.push(Reason { ok: false, text: format!("Based in {}, role is in {}", canonical_city(c), canonical_city(r)) });
            3.0
        }
        _ => 8.0,
    };
    if let Some(days) = cand.notice_days {
        reasons.push(Reason {
            ok: days <= 30,
            text: if days == 0 { "Immediate joiner".into() } else { format!("Notice period {days} days") },
        });
    }
    let total = (skill_points + exp_points + loc_points).round().clamp(0.0, 100.0) as u8;
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
        return TextResult::Unsupported;
    };
    match text {
        Ok(t) => TextResult::Text(crate::sources::normalize::truncate_chars(t.trim(), MAX_TEXT_CHARS)),
        Err(e) => TextResult::Failed(e),
    }
}

fn docx_text(bytes: &[u8]) -> Result<String, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| e.to_string())?
        .take(MAX_ATTACHMENT_BYTES as u64)
        .read_to_string(&mut xml)
        .map_err(|e| e.to_string())?;
    let with_breaks = xml.replace("</w:p>", "\n").replace("<w:tab/>", "\t").replace("<w:br/>", "\n");
    Ok(crate::sources::normalize::html_to_text(&with_breaks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_extract_requirement_and_candidate() {
        let r = rules_requirement(
            "Requirement: Java Developer",
            "Need 3 developers with Java, Spring Boot and Microservices. 5-8 yrs. Location Pune.",
            Some("capgemini.com"),
        );
        assert_eq!(r.role_title, "Java Developer");
        assert_eq!(r.client.as_deref(), Some("capgemini"));
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
        let (s, reasons) = score(&req, &good);
        assert_eq!(s, 80); // 40 skills + 25 exp + 15 loc
        assert!(reasons.iter().any(|r| r.ok && r.text.starts_with("Has 2/3")));
        assert!(reasons.iter().any(|r| !r.ok && r.text == "Missing: kafka"));

        let weak = CandidateFields { skills: vec!["python".into()], total_experience: Some(2.0), location: Some("Chennai".into()), ..good };
        assert!(score(&req, &weak).0 < MIN_MATCH_SCORE);
    }

    #[test]
    fn word_boundaries_and_canonical_names() {
        assert!(contains_word("knows .net and c#", ".net"));
        assert!(!contains_word("javascript", "java"));
        assert_eq!(canonical_skill("K8s"), "kubernetes");
        assert_eq!(canonical_city("Gurugram, Haryana"), "Delhi NCR");
    }
}
