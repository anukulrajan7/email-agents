use super::*;

/// Reads one email into records. Safe to run again: a source that already has a record is left as it is.
pub async fn record_message(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<Run> {
    let db = &state.db;
    let msg: Option<MessageRow> = sqlx::query_as(
        "SELECT subject, body_text, sender_address, original_sender_address, received_at, direction, suppressed
         FROM email_messages WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?;
    let Some(msg) = msg else { return Ok(Run::default()) };
    if msg.direction != "inbound" || msg.suppressed {
        return Ok(Run::default());
    }
    let received_at = msg.received_at;
    let cfg = settings::load(db, ws).await?;
    let (people, aliases) = load_people(db, ws).await?;
    let group = crate::platform::flags::mode(db, &state.config, ws, crate::platform::flags::Key::Handoff).await?.acts();
    let hint_year: i64 = clock::month_of(received_at, cfg.tz_offset_minutes)[..4].parse().unwrap_or(2026);
    let (units, waiting) = load_units(db, message_id, &msg).await?;
    // Files still being read: the run happens again when they are, and no model is paid for in the meantime.
    if waiting {
        return Ok(Run { waiting_for_files: true, ..Default::default() });
    }
    let has_files = units.iter().any(|u| u.source == "attachment");
    // Words alone, with no figure and no file, hold no timesheet.
    if !has_files && !units.iter().any(|u| u.text.chars().any(|c| c.is_ascii_digit())) {
        return Ok(Run::default());
    }
    let mut run = Run::default();
    let mut file_record_made = false;

    // Second pass: if the first one left nothing (a body that only says "see attached" next to files with no hours),
    // the body is recorded as empty so that someone looks, and the email is not read again and again.
    for force in [false, true] {
        if force && (!run.records.is_empty() || run.deferred) {
            break;
        }
        for unit in units.iter().take(if force { 1 } else { usize::MAX }) {
            if let Some(existing) = existing_by_source(db, ws, &unit.source_key).await? {
                file_record_made |= unit.source == "attachment";
                run.records.push(existing);
                continue;
            }
            let read = if unit.unreadable {
                Read { sheet: Sheet::default(), cost: 0.0, issues: vec![], deferred: false }
            } else {
                let allow_model = !(unit.source == "pasted" && has_files);
                read_text(state, ws, &msg.subject, &unit.text, hint_year, cfg.hours_tolerance, allow_model).await
            };
            if read.deferred {
                run.deferred = true;
                continue;
            }
            if read.cost > 0.0 {
                tracing::info!(%message_id, cost_usd = read.cost, "timesheet read with a model");
            }
            let Read { sheet, issues: read_issues, .. } = read;
            let empty = !sheet.has_data();
            if !force && unit.source == "pasted" && empty && has_files {
                continue;
            }
            if !force && unit.source == "attachment" && empty && !unit.unreadable && !NAME_CUE.is_match(unit.filename.as_deref().unwrap_or_default()) {
                continue;
            }
            // A subject that says "Fwd:" or "Fw:" marks a forward even when the mail carries no original sender: whoever it came from, a sheet
            // that names nobody is not theirs.
            let original = msg.original_sender_address.as_deref().map(address_of).or_else(|| parse::is_forward(&msg.subject).then(|| address_of(&msg.sender_address)));
            let m = parse::match_person(&people, &aliases, &sheet.identity, &address_of(&msg.sender_address), original.as_deref());
            let (period, assumed) = match sheet.period.clone() {
                Some(p) => (p, false),
                None => {
                    let month = period_of(&msg.subject, received_at, cfg.tz_offset_minutes, cfg.timesheet_grace_days);
                    (parse::month_dates(&month).unwrap_or_else(|| (clock::date_string(received_at.div_euclid(clock::DAY_MS)), clock::date_string(received_at.div_euclid(clock::DAY_MS)))), true)
                }
            };
            let who_text = match (&sheet.identity.name, &sheet.identity.code) {
                (Some(n), Some(c)) => format!("{n}, ID {c}"),
                (Some(n), None) => n.clone(),
                (None, Some(c)) => format!("ID {c}"),
                (None, None) => format!("no name or ID in the sheet; sent by {}", address_of(&msg.sender_address)),
            };
            let (total, mut issues) = assess(&sheet, &period, assumed, cfg.hours_tolerance, m.person_id.is_some(), &who_text);
            issues.extend(read_issues);
            if m.ambiguous {
                issues.push(note("ambiguous_person", "More than one person has this name."));
            }
            if unit.unreadable {
                issues.retain(|i| i.code != "no_hours");
                issues.push(check("unreadable", format!("The file {} could not be read.", unit.filename.as_deref().unwrap_or("attached"))));
            }
            let status = if issues.iter().any(|i| i.severity == CHECK) { "needs_check" } else { "recorded" };
            let draft = Draft {
                roster: &people,
                group,
                ws,
                message_id,
                unit,
                msg: &msg,
                sheet: &sheet,
                person_id: m.person_id.clone(),
                method: m.method,
                confidence: m.person_id.as_ref().map(|_| m.confidence),
                period,
                assumed,
                total,
                issues,
                status,
                client: sheet.identity.client.clone(),
            };
            let saved = insert_record(db, draft, people.iter().find(|p| Some(&p.id) == m.person_id.as_ref()).map(|p| p.name.clone())).await?;
            file_record_made |= unit.source == "attachment" && !unit.unreadable;
            run.records.push(saved);
        }
    }
    if file_record_made {
        retire_empty_pasted(db, ws, message_id).await?;
    }
    Ok(run)
}
