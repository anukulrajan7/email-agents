//! Golden drafts: the final text of six typical replies after the post-processing every draft goes through
//! (open roles, filler removal, team notes, "we'll confirm" rewrite, the owner's own sign-off). The model's
//! raw output is canned, so the snapshot shows exactly what changed. Prompt rules are pinned too.

use email_ai_api::{
    ai::{DRAFT_PROMPT_VERSION, openrouter::draft_system_prompt},
    replies::gap_resolver::{Opening, Sources, finalize_text},
    replies::signoff,
};

const SIGNATURE: &str = "Priya Nair\nHR Team, Acme";

fn role(title: &str, skills: &[&str], location: &str) -> Opening {
    Opening { id: "req_1".into(), role_title: title.into(), location: Some(location.into()), skills_json: serde_json::json!(skills).to_string() }
}

fn final_text(raw: &str, category: &str, subject: &str, email: &str, notes: &[&str], openings: &[Opening]) -> String {
    let notes: Vec<String> = notes.iter().map(|n| n.to_string()).collect();
    let src = Sources { ws: "w", staffing: true, subject, email_body: email, category, notes: &notes };
    let (mut text, _, _) = finalize_text(raw, &src, openings, !openings.is_empty());
    signoff::replace_agent_sign_off(&mut text, "Aarav", SIGNATURE);
    text
}

#[test]
fn the_prompt_is_versioned_and_pins_the_rules() {
    assert_eq!(DRAFT_PROMPT_VERSION, "ack-draft-v4");
    let prompt = draft_system_prompt("HR Team");
    for rule in ["60 to 140 words", "Never write vague filler", "Never sign with an assistant's name", "clear next step"] {
        assert!(prompt.contains(rule), "{rule}");
    }
}

#[test]
fn job_seeker_with_a_matching_opening() {
    let raw = "Hello Ananth,\n\nThank you for reaching out about a React Native role.\n\nYou asked if there are any React Native openings: [NEEDS: current React Native job openings]. We will review what we have and get back to you.\n\nBest regards,\nAarav";
    let out = final_text(raw, "candidate_referral", "Looking for a job", "Are there any React Native openings?", &[], &[role("React Native Developer", &["React Native"], "Pune")]);
    assert_eq!(
        out,
        "Hello Ananth,\n\nThank you for reaching out about a React Native role.\n\nWe currently have an open role that may fit: React Native Developer (Pune). Let us know if you'd like us to consider you.\n\nBest regards,\nPriya Nair\nHR Team, Acme"
    );
}

#[test]
fn job_seeker_without_an_opening() {
    let raw = "Hello Ananth,\n\nThank you for reaching out about a React Native role.\n\nYou asked if there are any React Native openings: [NEEDS: current React Native job openings]. We will review what we have and get back to you.\n\nBest regards,\nAarav";
    let out = final_text(raw, "candidate_referral", "Looking for a job", "Are there any React Native openings?", &[], &[]);
    assert_eq!(
        out,
        "Hello Ananth,\n\nThank you for reaching out about a React Native role.\n\nThanks for sharing your profile. We don't have a matching opening right now; we've saved your details and, as soon as a matching role comes in, we'll send it to you.\n\nBest regards,\nPriya Nair\nHR Team, Acme"
    );
}

#[test]
fn client_requirement_keeps_its_questions_for_the_sender() {
    let raw = "Hello Ravi,\n\nThank you for the Java requirement. We will share matching profiles.\n\nCould you share the [NEEDS: location], the [NEEDS: budget] and the [NEEDS: start date]?\n\nBest regards,\nAarav";
    let out = final_text(raw, "client_requirement", "Requirement: Java Developer", "Need Java developers", &[], &[]);
    assert_eq!(
        out,
        "Hello Ravi,\n\nThank you for the Java requirement. We will share matching profiles.\n\nCould you please share the location, the budget and the start date?\n\nBest regards,\nPriya Nair\nHR Team, Acme"
    );
}

#[test]
fn leave_request_uses_the_teams_own_note() {
    let raw = "Hello Sam,\n\nWe have received your leave request for two weeks.\n\nThe approver for your team is [NEEDS: approver for the HR team].\n\nBest regards,\nAarav";
    let out = final_text(raw, "leave", "Leave", "Who approves my leave?", &["Leave approvals: the approver for the HR team is Meera Rao"], &[]);
    assert_eq!(
        out,
        "Hello Sam,\n\nWe have received your leave request for two weeks.\n\nLeave approvals: the approver for the HR team is Meera Rao.\n\nBest regards,\nPriya Nair\nHR Team, Acme"
    );
}

#[test]
fn invoice_question_never_invents_an_amount() {
    let raw = "Hello Dev,\n\nThanks for checking on the invoice.\n\nThe outstanding amount is [NEEDS: invoice amount] and it will be paid by Friday.\n\nBest regards,\nAarav";
    let out = final_text(raw, "finance_query", "Invoice", "What is the outstanding amount?", &[], &[]);
    assert_eq!(
        out,
        "Hello Dev,\n\nThanks for checking on the invoice.\n\nWe'll confirm the payment details shortly.\n\nBest regards,\nPriya Nair\nHR Team, Acme"
    );
}

#[test]
fn complaint_apologises_and_promises_only_to_confirm() {
    let raw = "Hello Ines,\n\nI am sorry the totals looked wrong. The cause was [NEEDS: root cause of the wrong totals]. The fix is expected [NEEDS: fix date].\n\nBest regards,\nAarav";
    let out = final_text(raw, "support_question", "Dashboard totals wrong", "What is the cause and when will it be fixed?", &[], &[]);
    assert_eq!(
        out,
        "Hello Ines,\n\nI am sorry the totals looked wrong.\n\nWe'll confirm the details and the timeline shortly.\n\nBest regards,\nPriya Nair\nHR Team, Acme"
    );
}
