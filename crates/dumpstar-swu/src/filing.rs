//! Unmatched mechanic wording files its own vocabulary-fix bead (sp-12rpwl).
//!
//! `catalog-check` collects the cards whose mechanic-like rules text derives no
//! mechanic ([`crate::check::mechanic_findings`]) and calls [`plan`] with the
//! state of what was already filed. A plan is at most one payload: `open` when
//! no bead for this class is open, `append` (only the findings not yet filed)
//! when one is. No findings clears the state, so the next finding opens a new
//! bead once the fix has landed. The payload is written to the outbox
//! (`$DUMPSTAR_HOME/filings/outbox/`) for whatever carries it to Spira's store.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const CLASS: &str = "mechanic-vocabulary";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Finding {
    pub card_id: String,
    pub rules_text: String,
    pub phrase: String,
}

/// What has been filed and not yet cleared: the (card id, phrase) pairs the open
/// bead already lists.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub filed: BTreeSet<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Open,
    Append,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filing {
    pub class: String,
    pub action: Action,
    pub repo: String,
    pub title: String,
    pub body: String,
    pub findings: Vec<Finding>,
}

const ACCEPTANCE: &str = "## Build\n\
Extend crates/dumpstar-swu/src/mechanics.rs to classify the phrases listed below; \
bump the official derivation_version in crates/dumpstar-swu/src/apply.rs so every \
card re-derives on the next run; add a fixture test per phrase, seen red first. \
Acceptance: the next catalog-check reports none of these cards.\n";

fn lines(findings: &[Finding]) -> String {
    let mut s = String::new();
    for f in findings {
        s.push_str(&format!(
            "- {} | phrase: \"{}\"\n  rules text: {}\n",
            f.card_id,
            f.phrase,
            f.rules_text.replace('\n', " / ")
        ));
    }
    s
}

/// The next state and the payload to file, if any.
pub fn plan(findings: &[Finding], state: &State) -> (State, Option<Filing>) {
    if findings.is_empty() {
        return (State::default(), None);
    }
    let key = |f: &Finding| (f.card_id.clone(), f.phrase.clone());
    let fresh: Vec<Finding> = findings
        .iter()
        .filter(|f| !state.filed.contains(&key(f)))
        .cloned()
        .collect();
    let mut next = state.clone();
    next.filed.extend(findings.iter().map(key));
    if fresh.is_empty() {
        return (next, None);
    }
    let (action, title, body) = if state.filed.is_empty() {
        (
            Action::Open,
            "mechanic vocabulary: classify unmatched rules wording".to_string(),
            format!("{ACCEPTANCE}\n## Cards\n{}", lines(&fresh)),
        )
    } else {
        (
            Action::Append,
            "mechanic vocabulary: more unmatched rules wording".to_string(),
            format!("## More cards\n{}", lines(&fresh)),
        )
    };
    (
        next,
        Some(Filing {
            class: CLASS.into(),
            action,
            repo: "dumpstar".into(),
            title,
            body,
            findings: fresh,
        }),
    )
}

/// Run [`plan`] against the state file under `dir`, writing the payload to
/// `dir/outbox/` and the new state back. Returns the payload written.
pub fn file(dir: &Path, findings: &[Finding]) -> std::io::Result<Option<Filing>> {
    let state_path = dir.join("mechanic-vocabulary.state.json");
    let state: State = std::fs::read(&state_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let (next, filing) = plan(findings, &state);
    if let Some(f) = &filing {
        let out = dir.join("outbox");
        std::fs::create_dir_all(&out)?;
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
        std::fs::write(
            out.join(format!("{CLASS}-{stamp}.json")),
            serde_json::to_vec_pretty(f).map_err(std::io::Error::other)?,
        )?;
    }
    if next != state {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            &state_path,
            serde_json::to_vec(&next).map_err(std::io::Error::other)?,
        )?;
    }
    Ok(filing)
}
