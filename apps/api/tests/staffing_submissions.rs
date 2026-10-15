//! Gap 2: submissions. Outbound mail to a client domain with a CV attached is a submission; it is linked to a
//! candidate and requirement (confident links automatically, ambiguous ones wait for a person); client replies
//! move the outcome; false positives are never counted.

use crate::staffing_common;

use axum::http::StatusCode;
use email_ai_api::hr::staffing::submissions::{self, Outcome};
use serde_json::json;
use staffing_common::*;

struct World {
    h: H,
    ws: String,
    priya_src: String,
    java_req: String,
    data_req: String,
}

async fn world() -> World {
    let h = harness().await;
    let ws = h.workspace(&["staffing"]).await;
    let priya_src = h.mailbox(&ws, "priya@acme.test").await;
    let other = h.mailbox(&ws, "intake@acme.test").await;
    h.person(&ws, json!({"name": "Priya Sharma", "emails": ["priya@acme.test"], "role": "recruiter", "source_id": priya_src,
                         "joining_date": "2026-03-02", "daily_target": 5, "weekly_target": 25})).await;

    // A requirement and a referral already read by the matching pipeline.
    let req_mail = h.ingest(&ws, &other, email("hr@capgemini.com", &["intake@acme.test"], "Java Developer, 5 yrs, Hyderabad", "We need a Java developer.", WED - 9 * DAY), "ti1").await.unwrap();
    let req2_mail = h.ingest(&ws, &other, email("hr@capgemini.com", &["intake@acme.test"], "Data Engineer, Pune", "We need a data engineer.", WED - 8 * DAY), "ti2").await.unwrap();
    let ref_mail = h.ingest(&ws, &other, email("friend@example.com", &["intake@acme.test"], "Referral: Rahul Verma", "Rahul is a strong Java developer.", WED - 7 * DAY), "ti3").await.unwrap();
    let (java_req, data_req) = ("req_java".to_string(), "req_data".to_string());
    for (id, mail, client, title) in [(&java_req, &req_mail, "Capgemini", "Java Developer"), (&data_req, &req2_mail, "Capgemini", "Data Engineer")] {
        sqlx::query("INSERT INTO requirements (id, workspace_id, message_id, client, role_title, source, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'manual', 1)")
            .bind(id).bind(&ws).bind(mail).bind(client).bind(title).execute(&h.state.db).await.unwrap();
    }
    sqlx::query("INSERT INTO candidates (id, workspace_id, message_id, name, email, source, created_at) VALUES ('cand_rahul', ?1, ?2, 'Rahul Verma', 'rahul@example.com', 'manual', 1)")
        .bind(&ws).bind(&ref_mail).execute(&h.state.db).await.unwrap();
    sqlx::query("INSERT INTO matches (id, workspace_id, requirement_id, candidate_id, score, created_at, updated_at) VALUES ('mat_1', ?1, 'req_java', 'cand_rahul', 90, 1, 1)")
        .bind(&ws).execute(&h.state.db).await.unwrap();
    World { h, ws, priya_src, java_req, data_req }
}

async fn list(w: &World, query: &str) -> serde_json::Value {
    let (s, v) = w.h.get(OWNER, &format!("{}/staffing/submissions{query}", api(&w.ws))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

#[tokio::test]
async fn sent_cv_mail_becomes_a_linked_submission_and_noise_does_not() {
    let w = world().await;
    let (h, ws, src) = (&w.h, &w.ws, &w.priya_src);
    let at = WED - 3 * HOUR;
    // 1. A real submission, 2. CV to a candidate's own Gmail, 3. CV to a colleague, 4. client mail with no attachment,
    // 5. client mail with an invoice, 6. a CV sent from someone who is not in the directory.
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Java Developer profile - Rahul Verma", "Hi, sharing Rahul's profile.", at), &["Rahul_Verma_CV.pdf"])), "t1").await.unwrap();
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["rahul@gmail.com"], "Your CV", "Please review", at + 1000), &["Rahul_Verma_CV.pdf"])), "t2").await.unwrap();
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["colleague@acme.test"], "Check this CV", "FYI", at + 2000), &["Someone_CV.pdf"])), "t3").await.unwrap();
    h.ingest(ws, src, sent(email("priya@acme.test", &["hr@capgemini.com"], "Following up", "Any update?", at + 3000)), "t4").await.unwrap();
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["accounts@capgemini.com"], "Invoice", "Invoice attached", at + 4000), &["invoice-sept.pdf"])), "t5").await.unwrap();
    let stranger = h.mailbox(ws, "stranger@elsewhere.test").await;
    h.ingest(ws, &stranger, sent(with_files(email("stranger@elsewhere.test", &["hr@capgemini.com"], "Profile", "x", at + 5000), &["Z_CV.pdf"])), "t6").await.unwrap();

    let r = submissions::scan_workspace(&h.state, ws, WED).await.unwrap();
    assert_eq!((r.scanned, r.created), (4, 1), "{r:?}");
    let v = list(&w, "").await;
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let s = &items[0];
    assert_eq!((s["person_name"].as_str(), s["client_domain"].as_str(), s["attachment_name"].as_str()), (Some("Priya Sharma"), Some("capgemini.com"), Some("Rahul_Verma_CV.pdf")));
    // Names, the matching record and the client all agree: linked without asking anyone.
    assert_eq!((s["link_status"].as_str(), s["candidate_id"].as_str(), s["requirement_id"].as_str()), (Some("auto"), Some("cand_rahul"), Some(w.java_req.as_str())));
    assert_eq!(s["client"], "Capgemini");

    // Idempotent: nothing is scanned twice, and the same CV re-sent in the same thread is not counted again.
    let again = submissions::scan_workspace(&h.state, ws, WED).await.unwrap();
    assert_eq!((again.scanned, again.created), (0, 0));
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Re: Java Developer profile - Rahul Verma", "Resending the profile.", at + 6000), &["rahul-verma-resume.pdf"])), "t1").await.unwrap();
    let dup = submissions::scan_workspace(&h.state, ws, WED).await.unwrap();
    assert_eq!((dup.scanned, dup.created), (1, 0));
    assert_eq!(list(&w, "").await["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn ambiguous_links_wait_for_a_person_and_false_positives_can_be_dismissed() {
    let w = world().await;
    let (h, ws, src) = (&w.h, &w.ws, &w.priya_src);
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Profile for your open positions", "Please find the profile attached.", WED - 2 * HOUR), &["Anita_Desai_CV.docx"])), "t7").await.unwrap();
    submissions::scan_workspace(&h.state, ws, WED).await.unwrap();

    let v = list(&w, "?link_status=needs_confirm").await;
    assert_eq!(v["needs_confirm"], 1);
    let s = &v["items"][0];
    assert_eq!((s["candidate_name"].as_str(), s["requirement_id"].is_null()), (Some("Anita Desai"), true));
    let options: Vec<&str> = s["options"]["requirements"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(options.contains(&w.java_req.as_str()) && options.contains(&w.data_req.as_str()), "{s}");

    // The recruiter's team lead is not in the directory here, but the owner confirms the right one.
    let id = s["id"].as_str().unwrap();
    let (st, done) = h.call("PATCH", &format!("{}/staffing/submissions/{id}", api(ws)), OWNER, Some(json!({"requirement_id": w.data_req}))).await;
    assert_eq!(st, StatusCode::OK, "{done}");
    assert_eq!((done["link_status"].as_str(), done["requirement_id"].as_str()), (Some("confirmed"), Some(w.data_req.as_str())));
    assert_eq!(list(&w, "?link_status=needs_confirm").await["needs_confirm"], 0);
    let (st, e) = h.call("PATCH", &format!("{}/staffing/submissions/{id}", api(ws)), OWNER, Some(json!({"requirement_id": "req_nope"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");

    // A false positive is dismissed and no longer counts.
    let (_, done) = h.call("PATCH", &format!("{}/staffing/submissions/{id}", api(ws)), OWNER, Some(json!({"dismissed": true}))).await;
    assert_eq!(done["status"], "dismissed");
    assert!(list(&w, "").await["items"].as_array().unwrap().is_empty());
    assert_eq!(list(&w, "?status=dismissed").await["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn client_replies_move_the_outcome() {
    let w = world().await;
    let (h, ws, src) = (&w.h, &w.ws, &w.priya_src);
    let at = WED - 6 * HOUR;
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Java Developer profile - Rahul Verma", "Profile attached.", at), &["Rahul_Verma_CV.pdf"])), "t1").await.unwrap();
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Data Engineer profile", "Profile attached.", at + 1000), &["Anita_Desai_CV.pdf"])), "t2").await.unwrap();
    h.ingest(ws, src, sent(with_files(email("priya@acme.test", &["hr@capgemini.com"], "Another profile", "Profile attached.", at + 2000), &["Kiran_Rao_CV.pdf"])), "t3").await.unwrap();
    submissions::scan_workspace(&h.state, ws, WED).await.unwrap();
    assert_eq!(submissions::update_outcomes(&h.state.db, ws, WED).await.unwrap(), 0, "no replies yet");

    // Rahul: an interview is arranged in the same thread, then the client declines.
    let m = h.ingest(ws, src, email("hr@capgemini.com", &["priya@acme.test"], "Re: Java Developer profile - Rahul Verma", "Thanks, please arrange an interview slot on Friday.", at + HOUR), "t1").await.unwrap();
    h.classify(&m, "general").await;
    // Anita: a reply in another thread, labelled submission_update, naming her.
    let m = h.ingest(ws, src, email("recruiter@capgemini.com", &["priya@acme.test"], "Anita Desai: selected", "We have selected Anita Desai, the offer letter will follow.", at + 2 * HOUR), "t9").await.unwrap();
    h.classify(&m, "submission_update").await;
    // Kiran: a labelled reply about someone else must not touch Kiran.
    let m = h.ingest(ws, src, email("recruiter@capgemini.com", &["priya@acme.test"], "Joshi update", "Unfortunately Meera Joshi was not suitable.", at + 2 * HOUR), "t10").await.unwrap();
    h.classify(&m, "submission_update").await;
    assert_eq!(submissions::update_outcomes(&h.state.db, ws, WED).await.unwrap(), 2);

    let by_name = |v: &serde_json::Value, name: &str| v["items"].as_array().unwrap().iter().find(|s| s["candidate_name"] == name).cloned().unwrap_or_else(|| panic!("{name}: {v}"));
    let v = list(&w, "").await;
    let rahul = by_name(&v, "Rahul Verma");
    assert_eq!((rahul["outcome"].as_str(), rahul["reached_interview"].as_bool()), (Some("interview"), Some(true)));
    let anita = by_name(&v, "Anita Desai");
    assert_eq!((anita["outcome"].as_str(), anita["reached_interview"].as_bool()), (Some("selected"), Some(true)));
    assert_eq!(by_name(&v, "Kiran Rao")["outcome"], "pending");

    // Later the client rejects Rahul after the interview: rejected, but he still counts as interviewed.
    h.ingest(ws, src, email("hr@capgemini.com", &["priya@acme.test"], "Re: Java Developer profile - Rahul Verma", "Unfortunately the panel is not moving forward.", at + 3 * HOUR), "t1").await.unwrap();
    submissions::update_outcomes(&h.state.db, ws, WED).await.unwrap();
    let rahul = by_name(&list(&w, "").await, "Rahul Verma");
    assert_eq!((rahul["outcome"].as_str(), rahul["reached_interview"].as_bool()), (Some("rejected"), Some(true)));

    // A person can set the outcome by hand; the reader then leaves it alone.
    let kid = by_name(&list(&w, "").await, "Kiran Rao")["id"].as_str().unwrap().to_string();
    let (st, k) = h.call("PATCH", &format!("{}/staffing/submissions/{kid}", api(ws)), OWNER, Some(json!({"outcome": "joined"}))).await;
    assert_eq!((st, k["outcome"].as_str()), (StatusCode::OK, Some("joined")));
    assert!(k["reached_interview"].as_bool().unwrap());
}

#[test]
fn outcome_phrases() {
    assert_eq!(submissions::classify_outcome("Interview scheduled for Friday"), Some(Outcome::Interview));
    assert_eq!(submissions::classify_outcome("Candidate was not selected"), Some(Outcome::Rejected));
    assert_eq!(submissions::classify_outcome("Selected for the next round"), Some(Outcome::Interview));
    assert_eq!(submissions::classify_outcome("We have selected him, offer letter shared"), Some(Outcome::Selected));
    assert_eq!(submissions::classify_outcome("Priya has joined us today"), Some(Outcome::Joined));
    assert_eq!(submissions::classify_outcome("Thanks for the profile"), None);
    assert!(submissions::is_cv("Rahul_CV.PDF") && !submissions::is_cv("invoice.pdf") && !submissions::is_cv("photo.png"));
    assert_eq!(submissions::org_label("hr.capgemini.com"), "capgemini");
    assert_eq!(submissions::org_label("tcs.co.uk"), "tcs");
}
