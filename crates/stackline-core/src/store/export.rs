//! Writing stored lines out as text, JSON lines or CSV.

use super::*;

pub(super) fn export(db: &Connection, conn: &str, f: &LogFilter, format: ExportFormat, out: &mut dyn Write) -> Result<u64> {
    let (w, p) = f.where_clause(conn, true);
    let mut st = db.prepare(&format!(
        "SELECT id, ts, stack, service, replica, node, source, stream, level, exc, lines, fp, text, raw
             FROM entries WHERE {w} ORDER BY ts ASC, id ASC"
    ))?;
    let mut rows = st.query(params_from_iter(p.iter()))?;
    let mut n = 0;
    if format == ExportFormat::Csv {
        writeln!(out, "time,level,stack,service,replica,node,stream,message")?;
    }
    while let Some(r) = rows.next()? {
        let e = row_to_entry(r)?;
        let time = chrono::DateTime::from_timestamp_micros(e.ts)
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
            .unwrap_or_default();
        match format {
            ExportFormat::Log => {
                writeln!(out, "{time} {:<5} {}.{} {}", e.level.name(), e.service, e.replica, e.text)?;
            }
            ExportFormat::Jsonl => {
                let v = serde_json::json!({
                    "time": time, "level": e.level.name(), "stack": e.stack, "service": e.service,
                    "replica": e.replica, "node": e.node, "stream": e.stream, "message": e.text,
                    "raw": e.raw.as_deref().and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()),
                });
                writeln!(out, "{v}")?;
            }
            ExportFormat::Csv => {
                writeln!(
                    out,
                    "{},{},{},{},{},{},{},{}",
                    time,
                    e.level.name(),
                    csv(&e.stack),
                    csv(&e.service),
                    csv(&e.replica),
                    csv(&e.node),
                    e.stream,
                    csv(&e.text)
                )?;
            }
        }
        n += 1;
    }
    Ok(n)
}

pub(super) fn csv(s: &str) -> String {
    // A cell starting like a formula is run by spreadsheet apps when the file
    // is opened; a log line must never do that, so it starts with ' instead.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}
