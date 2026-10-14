//! The part of an email a model should read: what the sender wrote *this time*.
//!
//! Quoted history ("On … wrote:", Outlook "From/Sent" blocks, "-----Original Message-----", `>` lines),
//! mobile footers, disclaimers and sign-offs are cut. Earlier messages reach a prompt only as a short
//! thread digest. Two rules keep this safe:
//! - Forwarded mail is kept whole (the forwarded content is what the sender is asking about).
//! - Inline replies (answers written between `>` quoted lines) keep the answers.
//!
//! If cutting would leave nothing, the whole cleaned text is used instead: never lose an email.

use std::sync::LazyLock;

use regex::Regex;

use crate::knowledge::tidy;

/// One email split into what is new and the sender's sign-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySplit {
    pub new_part: String,
    /// Closing phrase, name and contact block (kept for memory extraction: role, company).
    pub signature: String,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

static FORWARD_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^\W*(begin forwarded message|forwarded message|forwarded by .+|original forwarded message|weitergeleitete nachricht|message transféré|mensaje reenviado)\W*:?$")
});
static FORWARD_SUBJECT: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^\s*(re:\s*)*(fwd?|wg|tr|rv)\s*:"));
static ORIGINAL_MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^-{2,}\s*(original message|ursprüngliche nachricht|message d'origine|mensaje original|original-nachricht)\s*-{2,}$")
});
static UNDERSCORE_RULE: LazyLock<Regex> = LazyLock::new(|| re(r"^[_\-=]{8,}$"));
static HEADER_FROM: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^[*_ ]*(from|von|de|da|van)\s*:\s*[*_ ]*\S"));
static HEADER_WHEN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^[*_ ]*(sent|date|gesendet|datum|envoyé|fecha|verzonden)\s*:"));
static HEADER_TO: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^[*_ ]*(to|an|à|para|subject|betreff|objet|asunto|cc|aan|onderwerp)\s*:"));
static WROTE_START: LazyLock<Regex> = LazyLock::new(|| re(r"^(On|El|Le|Am|Op|Il|Em)\s"));
static WROTE_END: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(wrote|escribió|a écrit|schrieb|schreef|ha scritto|escreveu)\s*:\s*$"));
static MOBILE_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^(sent from my |sent from (outlook|mail|yahoo|proton|gmail|my)|get outlook for |sent via |sent with |envoyé de mon |gesendet von meinem )")
});
static DISCLAIMER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^(confidentiality notice|confidential(ity)? disclaimer|disclaimer\b|legal notice|this (e-?mail|message)( and any (attachments|files))?( transmitted)? (is|are|may be|contains?) (confidential|intended|privileged)|the information (contained )?in this (e-?mail|message)|please consider the environment|think before you print|this email has been scanned|if you (are not|have received this).{0,40}(intended recipient|in error))")
});
static CLOSING: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^(many thanks|thanks a lot|thanks and regards|thanks & regards|thank you|thanks|best regards|kind regards|warm regards|warmest regards|with regards|with thanks|best wishes|regards|best|cheers|sincerely|yours sincerely|yours faithfully|yours truly|cordially|rgds|br|mit freundlichen grüßen|cordialement|saludos|cumprimentos)[\s,.!:-]*$")
});

const NAME_PARTICLES: &[&str] = &["de", "van", "von", "der", "den", "la", "le", "el", "bin", "al", "da", "di", "du", "dos"];
/// A sign-off block is short: at most this many lines and characters after the closing phrase.
const SIGNATURE_MAX_LINES: usize = 8;
const SIGNATURE_MAX_CHARS: usize = 500;
/// Disclaimers can run long.
const DISCLAIMER_MAX_CHARS: usize = 2_500;
const SIGNATURE_STORED_CHARS: usize = 300;

fn has_content(lines: &[String]) -> bool {
    lines.iter().any(|l| !l.trim().is_empty())
}

/// `On Tue, Oct 13, 2026 at 11:30 AM Priya <p@x.com> wrote:` on one line, or wrapped over two or three.
/// Returns how many lines the header takes.
fn wrote_header_len(lines: &[&str], at: usize) -> Option<usize> {
    let first = lines[at].trim();
    if !WROTE_START.is_match(first) {
        return None;
    }
    let mut joined = String::new();
    for (n, line) in lines.iter().skip(at).take(3).enumerate() {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        if n > 0 {
            joined.push(' ');
        }
        joined.push_str(line);
        if joined.chars().count() > 400 {
            return None;
        }
        if WROTE_END.is_match(&joined) {
            return Some(n + 1);
        }
    }
    None
}

/// Outlook-style header block: `From:` plus a sent/date line plus a to/subject/cc line within a few lines.
fn is_header_block(lines: &[&str], at: usize) -> bool {
    if !HEADER_FROM.is_match(lines[at].trim()) {
        return false;
    }
    let window: Vec<&str> = lines.iter().skip(at + 1).take(7).map(|l| l.trim()).collect();
    window.iter().any(|l| HEADER_WHEN.is_match(l)) && window.iter().any(|l| HEADER_TO.is_match(l))
}

fn is_forward_marker(line: &str) -> bool {
    FORWARD_MARKER.is_match(line.trim())
}

fn is_name_like(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() || words.len() > 4 || line.chars().any(|c| c.is_ascii_digit() || c == '?' || c == '@') {
        return false;
    }
    words.iter().all(|w| {
        let w = w.trim_matches(|c: char| ",.;|-".contains(c));
        w.is_empty() || NAME_PARTICLES.contains(&w) || w.chars().next().is_some_and(char::is_uppercase)
    })
}

/// Index where a sign-off or footer starts, when what follows is short enough to be one.
fn signature_start(lines: &[String]) -> Option<usize> {
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if i == 0 || line.is_empty() || !has_content(&lines[..i]) {
            continue;
        }
        let rest = &lines[i + 1..];
        let rest_lines = rest.iter().filter(|l| !l.trim().is_empty()).count();
        let rest_chars: usize = rest.iter().map(|l| l.trim().chars().count()).sum();
        let short_tail = rest_lines <= SIGNATURE_MAX_LINES + 2 && rest_chars <= SIGNATURE_MAX_CHARS * 3 / 2;
        let is_sig = if line == "--" || MOBILE_FOOTER.is_match(line) {
            short_tail
        } else if DISCLAIMER.is_match(line) {
            rest_chars <= DISCLAIMER_MAX_CHARS
        } else if CLOSING.is_match(line) && line.chars().count() <= 32 {
            let block: Vec<&str> = rest.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
            let chars: usize = block.iter().map(|l| l.chars().count()).sum();
            !block.is_empty()
                && block.len() <= SIGNATURE_MAX_LINES
                && chars <= SIGNATURE_MAX_CHARS
                && is_name_like(block[0])
                && !block.iter().any(|l| l.ends_with('?'))
        } else {
            false
        };
        if is_sig {
            return Some(i);
        }
    }
    None
}

/// Splits `text` (an email body, markup already removed) into the new part and its sign-off.
/// `subject` tells forwards (kept whole) from replies.
pub fn split(subject: &str, text: &str) -> BodySplit {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n").replace('\u{a0}', " ");
    let lines: Vec<&str> = normalized.lines().collect();
    let forwarded_in_body = lines.iter().any(|l| is_forward_marker(l));
    let forwarded_subject = FORWARD_SUBJECT.is_match(subject);
    if forwarded_in_body {
        return BodySplit { new_part: tidy(&normalized), signature: String::new() };
    }

    let mut kept: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.starts_with('>') {
            i += 1;
            continue;
        }
        let cuttable = has_content(&kept);
        if let Some(n) = wrote_header_len(&lines, i) {
            let next = lines.iter().skip(i + n).map(|l| l.trim()).find(|l| !l.is_empty());
            if next.is_some_and(|l| l.starts_with('>')) {
                // Quote-prefixed history: whatever is not prefixed after it is the sender's inline answer.
                i += n;
                continue;
            }
            if cuttable {
                break;
            }
        }
        if cuttable && !forwarded_subject {
            if ORIGINAL_MESSAGE.is_match(line) {
                break;
            }
            if is_header_block(&lines, i) {
                if kept.last().is_some_and(|l| UNDERSCORE_RULE.is_match(l.trim())) {
                    kept.pop();
                }
                break;
            }
        }
        kept.push(lines[i].to_string());
        i += 1;
    }
    if kept.last().is_some_and(|l| UNDERSCORE_RULE.is_match(l.trim())) && lines.len() > kept.len() {
        kept.pop();
    }

    let (body, signature) = match signature_start(&kept) {
        Some(at) => (kept[..at].to_vec(), kept[at..].to_vec()),
        None => (kept, Vec::new()),
    };
    let new_part = tidy(&body.join("\n"));
    if new_part.chars().all(char::is_whitespace) {
        return BodySplit { new_part: tidy(&normalized), signature: String::new() };
    }
    let signature = crate::sources::normalize::truncate_chars(&tidy(&signature.join("\n")), SIGNATURE_STORED_CHARS);
    BodySplit { new_part, signature }
}

/// What the sender wrote this time. This is what classify, analyze, draft, the judges and memory read.
pub fn model_body(subject: &str, text: &str) -> String {
    split(subject, text).new_part
}

/// One earlier message of a conversation, for the digest.
pub struct DigestEntry {
    /// "Priya" / "us".
    pub who: String,
    pub received_at_ms: i64,
    pub subject: String,
    pub body: String,
}

const DIGEST_LINE_CHARS: usize = 180;

/// A short, oldest-first summary of earlier messages: `- Priya (2026-10-12): first 180 chars…`.
/// Newest entries win when the budget runs out. Empty when there are no entries.
pub fn thread_digest(entries: &[DigestEntry], max_chars: usize) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            let text = model_body(&e.subject, &e.body);
            let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if one_line.is_empty() {
                return None;
            }
            let date = time::OffsetDateTime::from_unix_timestamp(e.received_at_ms / 1000)
                .map(|t| t.date().to_string())
                .unwrap_or_default();
            Some(format!("- {} ({date}): {}", e.who, crate::sources::normalize::truncate_chars(&one_line, DIGEST_LINE_CHARS)))
        })
        .collect();
    let mut total: usize = lines.iter().map(|l| l.chars().count() + 1).sum();
    while total > max_chars && lines.len() > 1 {
        total -= lines.remove(0).chars().count() + 1;
    }
    let digest = lines.join("\n");
    crate::sources::normalize::truncate_chars(&digest, max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(text: &str) -> String {
        model_body("Re: Payroll question", text)
    }

    #[test]
    fn gmail_single_line_header_drops_the_quote() {
        let text = "Thanks, that solved it.\n\nOn Tue, Oct 13, 2026 at 11:30 AM Priya Sharma <priya@acme.com> wrote:\n> Where is my payslip?\n> I could not find it.\n";
        assert_eq!(body(text), "Thanks, that solved it.");
    }

    #[test]
    fn gmail_two_line_header_drops_the_quote() {
        let text = "Yes, Friday works.\n\nOn Tue, Oct 13, 2026 at 11:30 AM Priya Sharma\n<priya.sharma@acme-corporation.example.com> wrote:\n\n> Can we meet Friday?\n";
        assert_eq!(body(text), "Yes, Friday works.");
    }

    #[test]
    fn gmail_three_line_header_and_unprefixed_quote() {
        let text = "Approved.\n\nOn Tue, Oct 13, 2026 at 11:30 AM\nPriya Sharma\n<p@acme.com> wrote:\nCan you approve the leave?\nThanks\nPriya\n";
        assert_eq!(body(text), "Approved.");
    }

    #[test]
    fn apple_mail_and_localized_headers() {
        assert_eq!(body("Sounds good.\n\nOn Oct 13, 2026, at 11:30 AM, Priya <p@a.com> wrote:\n\nOld text here"), "Sounds good.");
        assert_eq!(body("Gracias.\n\nEl mar, 13 oct 2026 a las 11:30, Priya <p@a.com> escribió:\n\nTexto anterior"), "Gracias.");
        assert_eq!(body("Merci.\n\nLe 13 oct. 2026 à 11:30, Priya <p@a.com> a écrit :\n\nTexte précédent"), "Merci.");
    }

    #[test]
    fn outlook_from_sent_block_is_cut() {
        let text = "Please see my answer below.\n\nRegards,\nPriya\n\nFrom: Meera Rao <meera@acme.com>\nSent: Tuesday, October 13, 2026 11:30 AM\nTo: Priya Sharma <priya@acme.com>\nCc: HR <hr@acme.com>\nSubject: RE: Payroll question\n\nOlder body that must not be read.";
        let out = body(text);
        assert!(out.contains("Please see my answer below."), "{out}");
        assert!(!out.contains("Older body") && !out.contains("From:") && !out.contains("Sent:"), "{out}");
    }

    #[test]
    fn outlook_underscore_rule_before_the_block_is_dropped() {
        let text = "Done.\n\n________________________________\nFrom: Meera <m@acme.com>\nSent: Tuesday, October 13, 2026 11:30 AM\nTo: Priya <p@acme.com>\nSubject: Hi\n\nOld";
        assert_eq!(body(text), "Done.");
    }

    #[test]
    fn original_message_marker_is_cut() {
        let text = "Got it, thanks.\n\n-----Original Message-----\nFrom: Meera\nSent: Monday\nSubject: Payroll\n\nOld question?";
        assert_eq!(body(text), "Got it, thanks.");
    }

    #[test]
    fn quoted_lines_are_dropped_without_a_header() {
        let text = "I agree with this.\n> We should move the deadline.\n> To Friday.\nMe too.";
        assert_eq!(body(text), "I agree with this.\nMe too.");
    }

    #[test]
    fn inline_replies_keep_the_answers() {
        let text = "On Tue, Oct 13, 2026 at 11:30 AM Support <support@acme.com> wrote:\n\n> 1. What is your employee ID?\nIt is E-4471.\n\n> 2. Which month is the payslip for?\nSeptember 2026, please.\n\n> Thanks,\n> Support\n";
        let out = body(text);
        assert!(out.contains("It is E-4471."), "{out}");
        assert!(out.contains("September 2026, please."), "{out}");
        assert!(!out.contains("What is your employee ID") && !out.contains("Which month"), "{out}");
    }

    #[test]
    fn inline_replies_with_a_leading_line_before_the_header() {
        let text = "Answers inline.\n\nOn Tue, Oct 13, 2026 at 11:30 AM Support <support@acme.com> wrote:\n> Is it urgent?\nYes, needed by Monday.\n> Who approves?\nMy manager, Dev Patel.\n";
        let out = body(text);
        assert!(out.contains("Answers inline.") && out.contains("Yes, needed by Monday.") && out.contains("My manager, Dev Patel."), "{out}");
        assert!(!out.contains("Is it urgent") && !out.contains("wrote:"), "{out}");
    }

    #[test]
    fn forwarded_mail_is_kept_whole() {
        let text = "FYI, can you handle this?\n\n---------- Forwarded message ---------\nFrom: Priya <priya@acme.com>\nDate: Mon, Oct 12, 2026 at 9:00 AM\nSubject: Refund not received\nTo: boss@acme.com\n\nHi, I was charged twice for order 4471.\nPlease refund the extra 2,400 INR.\n\nThanks,\nPriya";
        let out = model_body("Fwd: Refund not received", text);
        assert!(out.contains("FYI, can you handle this?"), "{out}");
        assert!(out.contains("charged twice for order 4471") && out.contains("2,400 INR") && out.contains("From: Priya"), "{out}");
    }

    #[test]
    fn outlook_forward_without_a_marker_keeps_the_forwarded_content() {
        let text = "Please handle.\n\nFrom: Priya <priya@acme.com>\nSent: Monday, October 12, 2026 9:00 AM\nTo: boss@acme.com\nSubject: Refund\n\nI was charged twice for order 4471.";
        let out = model_body("FW: Refund", text);
        assert!(out.contains("Please handle.") && out.contains("charged twice for order 4471"), "{out}");
    }

    #[test]
    fn forward_with_nothing_above_the_header_keeps_everything() {
        let text = "From: Priya <priya@acme.com>\nSent: Monday, October 12, 2026 9:00 AM\nTo: boss@acme.com\nSubject: Refund\n\nI was charged twice for order 4471.";
        let out = body(text);
        assert!(out.contains("charged twice for order 4471"), "{out}");
    }

    #[test]
    fn only_quoted_text_falls_back_to_the_whole_email() {
        let text = "> Please send the invoice.\n> By Friday.";
        assert!(model_body("Re: invoice", text).is_empty() || model_body("Re: invoice", text).contains("Please send the invoice"));
        let only_header = "On Mon, Oct 12, 2026 at 9:00 AM Priya <p@a.com> wrote:\n> Please send the invoice.";
        assert!(model_body("Re: invoice", only_header).contains("Please send the invoice"));
    }

    #[test]
    fn signature_delimiter_and_mobile_footers() {
        assert_eq!(body("Will do.\n\n-- \nPriya Sharma\nHR Manager, Acme\n+91 98765 43210"), "Will do.");
        assert_eq!(body("Will do.\n\nSent from my iPhone"), "Will do.");
        assert_eq!(body("Will do.\n\nGet Outlook for iOS <https://aka.ms/o0ukef>"), "Will do.");
    }

    #[test]
    fn sign_off_block_is_cut_and_kept_apart() {
        let text = "Can you share the Q3 report?\n\nBest regards,\nPriya Sharma\nSenior HR Manager | Acme Corp\nM: +91 98765 43210\nwww.acme.example";
        let s = split("Q3 report", text);
        assert_eq!(s.new_part, "Can you share the Q3 report?");
        assert!(s.signature.contains("Senior HR Manager") && s.signature.contains("Priya Sharma"), "{}", s.signature);
    }

    #[test]
    fn a_closing_word_followed_by_more_request_is_not_a_signature() {
        let text = "Hello.\nThanks,\nPlease send the signed offer letter by Friday.\nAlso confirm the joining date?";
        let out = body(text);
        assert!(out.contains("signed offer letter") && out.contains("joining date"), "{out}");
        let text = "Hi\nThanks\nCan you also send the form?";
        assert!(body(text).contains("send the form"));
    }

    #[test]
    fn disclaimers_are_cut_but_the_message_stays() {
        let text = "Please confirm the shift change.\n\nThis email and any attachments are confidential and intended solely for the addressee. If you are not the intended recipient, please delete it.";
        assert_eq!(body(text), "Please confirm the shift change.");
        let text = "The vendor said this message is confidential, so please keep the pricing private and confirm the amount.";
        assert_eq!(body(text), text);
    }

    #[test]
    fn a_mid_message_dash_line_is_not_a_signature() {
        let text = "Part one of the request.\n--\nPart two of the request, which is long enough to matter. It asks for the vendor list, the budget sheet, the approval chain, the audit trail, the exception report, the hiring plan, the headcount forecast and the travel policy.\nPlease reply with all of it by Monday morning so that finance can close the quarter without surprises or delays.\nAlso include the notes from the last review.\nAnd the sign-off sheet.\nAnd the signed contract copies.\nAnd the compliance checklist.\nAnd the training records.\nAnd the leave balance report.\nAnd the attendance export.\nAnd the shift roster for October.\nAnd the open positions list.\nAnd the offer letters sent this month.\nAnd the exit interview notes.\nAnd the final settlement sheet.";
        let out = body(text);
        assert!(out.contains("final settlement sheet") && out.contains("Part two"), "{out}");
    }

    #[test]
    fn plain_text_without_quotes_is_unchanged() {
        let text = "Hi team,\n\nThe invoice for October is attached. Please pay by the 15th.\n\nRegards,\nMeera";
        assert_eq!(body(text), "Hi team,\n\nThe invoice for October is attached. Please pay by the 15th.");
        let text = "Hello,\nWhere is order 5521?";
        assert_eq!(body(text), text);
    }

    #[test]
    fn crlf_and_nbsp_are_handled() {
        let text = "Thanks!\r\n\r\nOn Tue, Oct 13, 2026 at 11:30\u{a0}AM Priya <p@a.com> wrote:\r\n> hi\r\n";
        assert_eq!(body(text), "Thanks!");
    }

    /// A customer reply from Gmail's plain-text part: two-line header, quoted thread, signature above it.
    const GMAIL_THREAD: &str = "Hi Anita,\n\nThanks for the quick turnaround. The corrected invoice looks right now, so please go ahead and\nschedule the payment for the 28th. One more thing: can you also resend the GST certificate?\n\nRegards,\nRohan Mehta\nFinance Lead, Brightwave Pvt Ltd\n+91 98200 11223\n\nOn Mon, Oct 12, 2026 at 6:41 PM Anita Desai <anita.desai@vendorco.example.com>\nwrote:\n\n> Hi Rohan,\n>\n> Please find the corrected invoice attached (INV-2291, 1,18,000 INR).\n> Let me know if anything else needs changing.\n>\n> On Mon, Oct 12, 2026 at 11:02 AM Rohan Mehta <rohan@brightwave.example.com> wrote:\n>\n>> The invoice total is wrong, it should be 1,18,000 and not 1,81,000.\n";

    #[test]
    fn real_gmail_thread_keeps_the_two_asks_and_drops_everything_else() {
        let out = model_body("Re: Invoice INV-2291", GMAIL_THREAD);
        assert!(out.contains("schedule the payment for the 28th") && out.contains("resend the GST certificate?"), "{out}");
        for gone in ["1,18,000", "INV-2291, ", "wrote:", "Finance Lead", "98200"] {
            assert!(!out.contains(gone), "{gone} should be gone: {out}");
        }
        assert!(out.chars().count() < GMAIL_THREAD.chars().count() / 2);
    }

    /// Outlook desktop: top reply, signature with banner, disclaimer, then the From/Sent block with the old mail.
    const OUTLOOK_THREAD: &str = "Hi team,\n\nPlease approve leave for Karan from 20 to 24 October. His backup is Nisha.\n\nThanks,\nMeera Rao\nHR Business Partner\nACME India | Hyderabad\nT: +91 40 5555 0101\n\nThis email and any attachments are confidential and intended solely for the use of the individual to whom it is addressed.\n\nFrom: Karan Nair <karan.nair@acme.example.com>\nSent: Monday, October 12, 2026 4:15 PM\nTo: Meera Rao <meera.rao@acme.example.com>\nCc: HR Desk <hr@acme.example.com>\nSubject: Leave request 20-24 Oct\n\nHi Meera,\nI would like to take leave from 20 to 24 October for a family function.\n";

    #[test]
    fn real_outlook_thread_keeps_the_request_only() {
        let out = model_body("RE: Leave request 20-24 Oct", OUTLOOK_THREAD);
        assert_eq!(out, "Hi team,\n\nPlease approve leave for Karan from 20 to 24 October. His backup is Nisha.");
    }

    /// An Outlook forward: a one-line note on top, the customer's complaint below, with their own signature.
    const OUTLOOK_FORWARD: &str = "Can you take this one? Customer is upset.\n\n-----Original Message-----\nFrom: Sunita Verma <sunita@customer.example.com>\nSent: Tuesday, October 13, 2026 9:02 AM\nTo: support@acme.example.com\nSubject: Third time asking: refund for order 88121\n\nThis is the third time I am writing. Refund of 3,499 INR for order 88121 is still pending.\n\nRegards,\nSunita Verma\n";

    #[test]
    fn real_outlook_forward_keeps_the_customer_complaint() {
        let out = model_body("FW: Third time asking: refund for order 88121", OUTLOOK_FORWARD);
        assert!(out.contains("Can you take this one?"), "{out}");
        assert!(out.contains("third time I am writing") && out.contains("3,499 INR") && out.contains("Sunita Verma"), "{out}");
    }

    #[test]
    fn digest_is_short_oldest_first_and_bounded() {
        let entries = vec![
            DigestEntry { who: "Priya".into(), received_at_ms: 1_791_800_000_000, subject: "Payroll".into(), body: "Where is my payslip for September?\n\nOn Mon, Oct 12, 2026 at 9:00 AM Support wrote:\n> old".into() },
            DigestEntry { who: "us".into(), received_at_ms: 1_791_890_000_000, subject: "Re: Payroll".into(), body: "I will send it by Friday.\n\n-- \nSupport".into() },
            DigestEntry { who: "Priya".into(), received_at_ms: 1_791_900_000_000, subject: "Re: Payroll".into(), body: String::new() },
        ];
        let d = thread_digest(&entries, 800);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines.len(), 2, "empty messages are skipped: {d}");
        assert!(lines[0].starts_with("- Priya (2026-10-") && lines[0].ends_with("payslip for September?"), "{d}");
        assert!(lines[1].contains("us (") && lines[1].ends_with("I will send it by Friday."), "{d}");
        let tight = thread_digest(&entries, lines[1].len() + 2);
        assert_eq!(tight.lines().count(), 1);
        assert!(tight.contains("I will send it"), "newest wins: {tight}");
        assert_eq!(thread_digest(&[], 800), "");
    }
}
