//! Keywords on assets. Every (asset, keyword) pair has one `source`: `user`, or `ai:<name>` for
//! machine labels (`ai:claude`). A user keyword always wins over an AI one; AI labels can be
//! replaced or cleared without touching the user's own keywords.
//!
//! The labelling itself happens in the desktop frontend (Claude via the owner's API key, only
//! when the user asks); this module stores what comes back.

use super::now;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub const USER: &str = "user";
pub const AI_PREFIX: &str = "ai:";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetKeyword {
    pub name: String,
    pub source: String,
    pub confidence: Option<f64>,
}

/// Trim, collapse whitespace; `None` for empty or absurdly long input.
pub fn normalize(name: &str) -> Option<String> {
    let s = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty() && s.chars().count() <= 64).then_some(s)
}

fn check_source(source: &str) -> anyhow::Result<()> {
    if source == USER || (source.starts_with(AI_PREFIX) && source.len() > AI_PREFIX.len()) {
        Ok(())
    } else {
        anyhow::bail!("keyword source must be 'user' or 'ai:<name>', got '{source}'")
    }
}

fn keyword_id(conn: &Connection, name: &str) -> anyhow::Result<i64> {
    conn.prepare_cached("INSERT INTO keywords (name) VALUES (?1) ON CONFLICT (name) DO NOTHING")?.execute([name])?;
    Ok(conn.prepare_cached("SELECT id FROM keywords WHERE name = ?1")?.query_row([name], |r| r.get(0))?)
}

fn insert(conn: &Connection, asset: i64, keyword: i64, source: &str, confidence: Option<f64>, stamp: &str) -> anyhow::Result<usize> {
    Ok(conn
        .prepare_cached(
            "INSERT INTO asset_keywords (asset_id, keyword_id, source, confidence, added_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (asset_id, keyword_id) DO UPDATE SET
                source = CASE WHEN asset_keywords.source = 'user' THEN 'user' ELSE excluded.source END,
                confidence = CASE WHEN asset_keywords.source = 'user' THEN asset_keywords.confidence ELSE excluded.confidence END",
        )?
        .execute(params![asset, keyword, source, confidence, stamp])?)
}

/// Add keywords to assets. Returns the number of (asset, keyword) rows written.
pub fn add(conn: &Connection, asset_ids: &[i64], names: &[String], source: &str) -> anyhow::Result<usize> {
    check_source(source)?;
    let names: Vec<String> = names.iter().filter_map(|n| normalize(n)).collect();
    let stamp = now();
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for name in &names {
        let kw = keyword_id(&tx, name)?;
        for a in asset_ids {
            n += insert(&tx, *a, kw, source, None, &stamp)?;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Remove keywords from assets, whatever their source. Returns rows removed.
pub fn remove(conn: &Connection, asset_ids: &[i64], names: &[String]) -> anyhow::Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for name in names {
        let Some(name) = normalize(name) else { continue };
        for a in asset_ids {
            n += tx
                .prepare_cached("DELETE FROM asset_keywords WHERE asset_id = ?1 AND keyword_id = (SELECT id FROM keywords WHERE name = ?2)")?
                .execute(params![a, name])?;
        }
    }
    tx.commit()?;
    prune_unused(conn)?;
    Ok(n)
}

/// Remove every keyword whose source starts with `prefix` (e.g. `ai:` for all AI labels) from
/// these assets. User keywords are never touched.
pub fn clear_source(conn: &Connection, asset_ids: &[i64], prefix: &str) -> anyhow::Result<usize> {
    if prefix.is_empty() || USER.starts_with(prefix) {
        anyhow::bail!("refusing to clear user keywords");
    }
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for a in asset_ids {
        n += tx
            .prepare_cached("DELETE FROM asset_keywords WHERE asset_id = ?1 AND source <> 'user' AND substr(source, 1, length(?2)) = ?2")?
            .execute(params![a, prefix])?;
    }
    tx.commit()?;
    prune_unused(conn)?;
    Ok(n)
}

/// One asset's labels from a labeller.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetLabels {
    pub id: i64,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub confidences: Vec<f64>,
}

/// Store machine labels under `source` (`ai:<name>`). With `replace`, the assets' previous labels
/// from that source are removed first. User keywords are untouched either way, and a label that
/// is already a user keyword stays a user keyword.
pub fn apply_labels(conn: &Connection, labels: &[AssetLabels], source: &str, replace: bool) -> anyhow::Result<usize> {
    check_source(source)?;
    if source == USER {
        anyhow::bail!("labels need an 'ai:<name>' source");
    }
    let stamp = now();
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for l in labels {
        if replace {
            tx.prepare_cached("DELETE FROM asset_keywords WHERE asset_id = ?1 AND source = ?2")?.execute(params![l.id, source])?;
        }
        for (i, k) in l.keywords.iter().enumerate() {
            let Some(k) = normalize(k) else { continue };
            let kw = keyword_id(&tx, &k)?;
            n += insert(&tx, l.id, kw, source, l.confidences.get(i).copied(), &stamp)?;
        }
    }
    tx.commit()?;
    prune_unused(conn)?;
    Ok(n)
}

pub fn of_asset(conn: &Connection, asset: i64) -> anyhow::Result<Vec<AssetKeyword>> {
    let mut stmt = conn.prepare_cached(
        "SELECT k.name, ak.source, ak.confidence FROM asset_keywords ak JOIN keywords k ON k.id = ak.keyword_id
         WHERE ak.asset_id = ?1 ORDER BY ak.source <> 'user', k.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([asset], |r| Ok(AssetKeyword { name: r.get(0)?, source: r.get(1)?, confidence: r.get(2)? }))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Drop keywords no asset uses any more.
pub fn prune_unused(conn: &Connection) -> anyhow::Result<usize> {
    Ok(conn.execute("DELETE FROM keywords WHERE NOT EXISTS (SELECT 1 FROM asset_keywords ak WHERE ak.keyword_id = keywords.id)", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn cat_with_assets(n: usize) -> (Catalog, Vec<i64>) {
        let cat = Catalog::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for _ in 0..n {
            cat.conn().execute("INSERT INTO assets (kind, added_at) VALUES ('raw', 'now')", []).unwrap();
            ids.push(cat.conn().last_insert_rowid());
        }
        (cat, ids)
    }

    fn names(cat: &Catalog, id: i64) -> Vec<(String, String)> {
        of_asset(cat.conn(), id).unwrap().into_iter().map(|k| (k.name, k.source)).collect()
    }

    #[test]
    fn add_remove_on_many_assets_case_insensitively() {
        let (cat, ids) = cat_with_assets(3);
        let c = cat.conn();
        assert_eq!(add(c, &ids[..2], &["Beach".into(), "  sunset   glow ".into(), "".into()], USER).unwrap(), 4);
        assert_eq!(names(&cat, ids[0]), vec![("Beach".into(), "user".into()), ("sunset glow".into(), "user".into())]);
        assert!(names(&cat, ids[2]).is_empty());
        // Same keyword in another case reuses the row and keeps the first spelling.
        add(c, &ids[2..], &["beach".into()], USER).unwrap();
        assert_eq!(names(&cat, ids[2]), vec![("Beach".into(), "user".into())]);
        let kw: i64 = c.query_row("SELECT COUNT(*) FROM keywords", [], |r| r.get(0)).unwrap();
        assert_eq!(kw, 2);
        assert_eq!(remove(c, &ids, &["BEACH".into()]).unwrap(), 3);
        assert_eq!(names(&cat, ids[0]), vec![("sunset glow".into(), "user".into())]);
        remove(c, &ids, &["sunset glow".into()]).unwrap();
        let kw: i64 = c.query_row("SELECT COUNT(*) FROM keywords", [], |r| r.get(0)).unwrap();
        assert_eq!(kw, 0, "unused keywords are pruned");
        assert!(add(c, &ids, &["x".into()], "robot").is_err());
    }

    #[test]
    fn ai_labels_never_override_or_clear_user_keywords() {
        let (cat, ids) = cat_with_assets(2);
        let c = cat.conn();
        add(c, &ids[..1], &["dog".into()], USER).unwrap();
        let labels = vec![
            AssetLabels { id: ids[0], keywords: vec!["dog".into(), "grass".into()], confidences: vec![] },
            AssetLabels { id: ids[1], keywords: vec!["street".into(), "night".into()], confidences: vec![0.9, 0.4] },
        ];
        apply_labels(c, &labels, "ai:claude", true).unwrap();
        assert_eq!(names(&cat, ids[0]), vec![("dog".into(), "user".into()), ("grass".into(), "ai:claude".into())]);
        let k = of_asset(c, ids[1]).unwrap();
        assert_eq!(k.iter().find(|k| k.name == "street").unwrap().confidence, Some(0.9));

        // Relabelling with replace swaps the AI set only.
        apply_labels(c, &[AssetLabels { id: ids[0], keywords: vec!["park".into()], confidences: vec![] }], "ai:claude", true).unwrap();
        assert_eq!(names(&cat, ids[0]), vec![("dog".into(), "user".into()), ("park".into(), "ai:claude".into())]);

        // A user adding an AI keyword promotes it to theirs.
        add(c, &ids[1..], &["night".into()], USER).unwrap();
        assert_eq!(clear_source(c, &ids, AI_PREFIX).unwrap(), 2);
        assert_eq!(names(&cat, ids[0]), vec![("dog".into(), "user".into())]);
        assert_eq!(names(&cat, ids[1]), vec![("night".into(), "user".into())]);
        assert!(clear_source(c, &ids, "user").is_err());
        assert!(apply_labels(c, &labels, USER, false).is_err());
    }
}
