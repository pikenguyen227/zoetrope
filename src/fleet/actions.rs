//! Hiding finished workers, and asking the collector to archive or delete
//! records: the whole fleet or one worker.
//!
//! The viewer never changes the adapter's records itself. The collector owns
//! them and holds them in memory too, so it would write back anything removed
//! under it. A confirmed request goes to the one file the collector named
//! (`ZOE_FLEET_REQUEST`) and the viewer exits; the collector carries it out,
//! collects afresh and starts a new viewer. Without a collector, the same
//! actions are the adapter's `--archive` and `--delete` commands.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::journal::Attempt;
use super::{Fleet, SessionKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Move the records into a backup beside them: recoverable.
    Archive,
    /// Remove the records permanently.
    Delete,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Delete => "delete",
        }
    }
}

/// One worker's records: a member session with every attempt joined to it,
/// or an attempt that never had a session. Removal is scoped by attempt, so
/// it never takes another attempt's records with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Session(SessionKey),
    Attempt(Attempt),
}

/// What the viewer hands the collector on exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub action: Action,
    /// `None`: the whole fleet.
    pub target: Option<Target>,
    /// The operator confirmed deleting a worker still registered.
    pub confirmed_registered: bool,
}

impl Request {
    /// The request file's contents, as `scripts/firstmate-fleet.py` reads them.
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "action": self.action.name(),
            "scope": if self.target.is_some() { "worker" } else { "all" },
        });
        match &self.target {
            Some(Target::Session(key)) => value["session"] = json!(key),
            Some(Target::Attempt(attempt)) => value["attempt"] = json!(attempt),
            None => {}
        }
        if self.confirmed_registered {
            value["confirmed_registered"] = json!(true);
        }
        value
    }
}

/// A worker as the prompts describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worker {
    pub target: Target,
    pub label: String,
    /// Not finished: the adapter still registers it, so the next collection
    /// brings it back.
    pub registered: bool,
    /// Lifecycle records its attempts own.
    pub records: usize,
}

/// A question or note that takes the next key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// Archive (`a`) or delete (`D`) everything.
    Clear,
    /// Delete everything, permanently (`y`).
    DeleteAll { members: usize, records: usize },
    /// Delete one worker, permanently (`y`).
    Delete(Worker),
    /// Any key dismisses it.
    Notice(String),
}

impl Prompt {
    /// What the header says: what is at stake, then the keys that answer.
    pub fn lines(&self) -> (String, String) {
        match self {
            Self::Clear => (
                "Clear the fleet and start fresh: archive moves the manifest and its journal \
                 into a backup-<time> directory beside them; delete removes them"
                    .into(),
                "a: archive all (restorable) · D: delete all · any other key: cancel".into(),
            ),
            Self::DeleteAll { members, records } => (
                format!(
                    "DELETE the manifest and its journal: {members} members, {records} lifecycle \
                     records and every checkpoint. Backups and transcripts stay. \
                     This cannot be undone."
                ),
                "y: delete everything · any other key: cancel".into(),
            ),
            Self::Delete(worker) => {
                let registered = if worker.registered {
                    format!(
                        "{} is STILL REGISTERED and will reappear on the next collection. ",
                        worker.label
                    )
                } else {
                    String::new()
                };
                (
                    format!(
                        "{registered}DELETE {}: its manifest entry, {} lifecycle records and its \
                         part of every checkpoint. Its transcript stays. This cannot be undone.",
                        worker.label, worker.records
                    ),
                    format!("y: delete {} · any other key: cancel", worker.label),
                )
            }
            Self::Notice(text) => (text.clone(), "any key: dismiss".into()),
        }
    }
}

const NO_COLLECTOR: &str = "No collector runs this viewer: archive or delete with \
    scripts/firstmate-fleet.py --archive / --delete (see docs/FLEET-USAGE.md)";

impl Fleet {
    /// Show or hide finished members and cards, and re-arrange what remains.
    pub fn toggle_finished(&mut self) {
        self.show_finished = !self.show_finished;
        self.rearrange = true;
        self.sync();
        // Cards coming back carry their history, not a burst of fresh chips.
        self.overview.chips.adopt_baseline(&self.overview.session);
    }

    /// The worker behind an overview node, as it stands today.
    pub fn worker(&self, id: &str) -> Option<Worker> {
        let crew = self.lifecycle.state_at(None);
        let joins = self.joins();
        let (target, label, attempts, registered) = if let Some((key, _)) = self.nodes.get(id) {
            let member = self.members.get(key)?;
            let attempts: BTreeSet<Attempt> = joins
                .iter()
                .filter(|(_, joined)| *joined == key)
                .map(|(attempt, _)| attempt.clone())
                .collect();
            let registered = !super::finished(key, member.retained, None, &crew, &joins);
            let label = member.spec.label.clone();
            (Target::Session(key.clone()), label, attempts, registered)
        } else {
            let attempt = self.cards.get(id)?;
            let registered = crew.get(attempt).is_none_or(|s| s.torn_down.is_none());
            let attempts = BTreeSet::from([attempt.clone()]);
            let label = attempt.task.clone();
            (
                Target::Attempt(attempt.clone()),
                label,
                attempts,
                registered,
            )
        };
        Some(Worker {
            records: self.lifecycle.count_for(&attempts),
            target,
            label,
            registered,
        })
    }

    fn selected_worker(&mut self) -> Option<Worker> {
        if !self.collector {
            self.prompt = Some(Prompt::Notice(NO_COLLECTOR.into()));
            return None;
        }
        let worker = self
            .overview
            .selected_agent_id()
            .and_then(|id| self.worker(&id));
        if worker.is_none() {
            self.prompt = Some(Prompt::Notice("Select a worker's card first".into()));
        }
        worker
    }

    /// `A`: archive the selected worker. It is recoverable, so it asks
    /// nothing; a worker still registered would come straight back, so that
    /// is refused. Returns whether the viewer exits to hand over the request.
    pub fn archive_selected(&mut self) -> bool {
        let Some(worker) = self.selected_worker() else {
            return false;
        };
        if worker.registered {
            self.prompt = Some(Prompt::Notice(format!(
                "{} is still registered: archive it once it finishes",
                worker.label
            )));
            return false;
        }
        self.request = Some(Request {
            action: Action::Archive,
            target: Some(worker.target),
            confirmed_registered: false,
        });
        true
    }

    /// `D`: ask to delete the selected worker.
    pub fn delete_selected(&mut self) {
        if let Some(worker) = self.selected_worker() {
            self.prompt = Some(Prompt::Delete(worker));
        }
    }

    /// `C`: ask whether to archive or delete everything.
    pub fn ask_clear(&mut self) {
        self.prompt = Some(if self.collector {
            Prompt::Clear
        } else {
            Prompt::Notice(NO_COLLECTOR.into())
        });
    }

    /// Answer the open prompt with a plain character, or `None` for any other
    /// key, which cancels. Returns whether the viewer exits to hand over the
    /// request.
    pub fn answer(&mut self, key: Option<char>) -> bool {
        let Some(prompt) = self.prompt.take() else {
            return false;
        };
        let request = match (prompt, key) {
            (Prompt::Clear, Some('a')) => Request {
                action: Action::Archive,
                target: None,
                confirmed_registered: false,
            },
            (Prompt::Clear, Some('D')) => {
                self.prompt = Some(Prompt::DeleteAll {
                    members: self.members.len(),
                    records: self.lifecycle.len(),
                });
                return false;
            }
            (Prompt::DeleteAll { .. }, Some('y')) => Request {
                action: Action::Delete,
                target: None,
                confirmed_registered: false,
            },
            (Prompt::Delete(worker), Some('y')) => Request {
                action: Action::Delete,
                target: Some(worker.target),
                confirmed_registered: worker.registered,
            },
            _ => return false,
        };
        self.request = Some(request);
        true
    }
}
