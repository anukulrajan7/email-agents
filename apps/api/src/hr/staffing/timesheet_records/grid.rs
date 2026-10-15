use super::*;

#[derive(sqlx::FromRow)]
struct GridPerson {
    id: String,
    name: String,
    employee_code: Option<String>,
    client: Option<String>,
    employee_type: String,
    joining_date: Option<String>,
}

fn expected_people_sql() -> &'static str {
    "SELECT id, name, employee_code, client, employee_type, joining_date FROM people WHERE workspace_id = ?1 AND active = 1 ORDER BY lower(name), id"
}

/// People by days for a month: one row per expected person (and per unmatched sheet), hours per day, a total and
/// a status. Replaced records never count.
pub async fn grid(db: &SqlitePool, ws: &str, month: &str, now: i64, a: &Access) -> sqlx::Result<Value> {
    let cfg = settings::load(db, ws).await?;
    let Some((first, last)) = parse::month_dates(month) else { return Ok(json!({ "month": month, "days": [], "rows": [] })) };
    let (d0, d1) = (clock::parse_date(&first).unwrap_or(0), clock::parse_date(&last).unwrap_or(0));
    let days: Vec<Value> = (d0..=d1)
        .map(|d| {
            let w = (d + 3).rem_euclid(7);
            json!({ "date": clock::date_string(d), "weekday": weekday_name(w), "weekend": w >= 5 })
        })
        .collect();
    let people: Vec<GridPerson> = sqlx::query_as(expected_people_sql()).bind(ws).fetch_all(db).await?;
    let any_billable = people.iter().any(|p| p.employee_type == "billable");
    let records: Vec<RecordRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE r.workspace_id = ?1 AND r.status != 'replaced' AND r.period_start <= ?3 AND r.period_end >= ?2 ORDER BY r.period_start, r.created_at"
    ))
    .bind(ws)
    .bind(&first)
    .bind(&last)
    .fetch_all(db)
    .await?;
    let due_at = clock::month_bounds(month, cfg.tz_offset_minutes).map_or(i64::MAX, |(_, end)| end + cfg.timesheet_grace_days * clock::DAY_MS);

    #[derive(Default)]
    struct Row {
        person_id: Option<String>,
        name: String,
        code: Option<String>,
        client: Option<String>,
        cells: HashMap<String, f64>,
        undated: f64,
        records: Vec<String>,
        check: bool,
    }
    let mut rows: Vec<Row> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    for p in &people {
        let started = p.joining_date.as_deref().is_none_or(|j| j.get(..7).unwrap_or(j) <= month);
        if !a.can_see(&p.id) {
            continue;
        }
        let has_record = records.iter().any(|r| r.person_id.as_deref() == Some(p.id.as_str()));
        if (any_billable && p.employee_type != "billable" && !has_record) || (!started && !has_record) {
            continue;
        }
        index.insert(p.id.clone(), rows.len());
        rows.push(Row { person_id: Some(p.id.clone()), name: p.name.clone(), code: p.employee_code.clone(), client: p.client.clone(), ..Default::default() });
    }
    for r in &records {
        if !visible(a, r) {
            continue;
        }
        let i = match r.person_id.as_ref().and_then(|p| index.get(p)) {
            Some(i) => *i,
            None => {
                rows.push(Row { person_id: r.person_id.clone(), name: r.linked_name.clone().or_else(|| r.person_name.clone()).unwrap_or_else(|| "Unknown".into()), code: r.employee_code.clone(), client: r.client.clone(), ..Default::default() });
                if let Some(p) = &r.person_id {
                    index.insert(p.clone(), rows.len() - 1);
                }
                rows.len() - 1
            }
        };
        let row = &mut rows[i];
        row.client = row.client.take().or_else(|| r.client.clone()).or_else(|| r.entries().iter().find_map(|e| e.project.clone()));
        row.records.push(r.id.clone());
        row.check |= r.status == "needs_check" || r.person_id.is_none();
        let entries = r.entries();
        // A sheet that spans months counts its undated hours once, in the month it starts.
        let starts_here = r.period_start.as_str() >= first.as_str() && r.period_start.as_str() <= last.as_str();
        if entries.is_empty() && starts_here {
            row.undated += r.total_hours.unwrap_or(0.0);
        }
        for e in entries {
            match e.date {
                Some(d) if d.as_str() >= first.as_str() && d.as_str() <= last.as_str() => *row.cells.entry(d).or_default() += e.hours,
                Some(_) => {}
                None if starts_here => row.undated += e.hours,
                None => {}
            }
        }
    }
    let (mut recorded, mut needs_check, mut missing) = (0, 0, 0);
    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let status = if r.records.is_empty() {
                if now >= due_at { "missing" } else { "pending" }
            } else if r.check {
                "needs_check"
            } else {
                "recorded"
            };
            match status {
                "recorded" => recorded += 1,
                "needs_check" => needs_check += 1,
                "missing" => missing += 1,
                _ => {}
            }
            let day_sum: f64 = r.cells.values().sum();
            let total = (day_sum + r.undated) * 100.0;
            json!({
                "person_id": r.person_id, "name": r.name, "employee_code": r.code, "client": r.client,
                "cells": r.cells, "total": (total.round() / 100.0), "status": status, "record_ids": r.records,
            })
        })
        .collect();
    Ok(json!({
        "month": month, "days": days, "rows": out, "due_at": iso(due_at.min(i64::MAX / 2)),
        "summary": { "recorded": recorded, "needs_check": needs_check, "missing": missing },
    }))
}
