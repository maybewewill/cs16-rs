//! Priority-ordered ReAPI-style pre/post hook chains across [`HookId`].

use std::collections::BTreeMap;

use cs_mod_api::{HookId, HookPhase};

#[derive(Debug, Clone)]
pub struct HookSubscriber {
    pub mod_id: String,
    pub priority: i32,
    pub order: usize,
}

#[derive(Debug, Default)]
pub struct HookTable {
    entries: BTreeMap<(HookId, HookPhase), Vec<HookSubscriber>>,
    seq: usize,
}

impl HookTable {
    pub fn register(&mut self, mod_id: &str, hook: HookId, phase: HookPhase, priority: i32) {
        let list = self.entries.entry((hook, phase)).or_default();
        if let Some(existing) = list.iter_mut().find(|s| s.mod_id == mod_id) {
            existing.priority = priority;
        } else {
            self.seq += 1;
            list.push(HookSubscriber {
                mod_id: mod_id.to_string(),
                priority,
                order: self.seq,
            });
        }
        list.sort_by_key(|s| (s.priority, s.order));
    }

    pub fn unregister_mod(&mut self, mod_id: &str) {
        for list in self.entries.values_mut() {
            list.retain(|s| s.mod_id != mod_id);
        }
    }

    pub fn subscribers(&self, hook: HookId, phase: HookPhase) -> Vec<String> {
        self.entries
            .get(&(hook, phase))
            .map(|list| list.iter().map(|s| s.mod_id.clone()).collect())
            .unwrap_or_default()
    }
}
