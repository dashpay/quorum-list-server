use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub struct QuorumList {
    pub list: Vec<QuorumListEntry>,
}

impl QuorumList {
    pub fn new() -> Self {
        Self {
            list: Vec::new(),
        }
    }

    pub fn add_entry(&mut self, entry: QuorumListEntry) {
        if !self.contains_quorum(&entry.quorum_hash) {
            self.list.push(entry);
        }
    }

    pub fn remove_entry(&mut self, quorum_hash: &[u8]) -> bool {
        if let Some(pos) = self.list.iter().position(|x| x.quorum_hash == quorum_hash) {
            self.list.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn contains_quorum(&self, quorum_hash: &[u8]) -> bool {
        self.list.iter().any(|entry| entry.quorum_hash == quorum_hash)
    }

    pub fn get_entry(&self, quorum_hash: &[u8]) -> Option<&QuorumListEntry> {
        self.list.iter().find(|entry| entry.quorum_hash == quorum_hash)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &QuorumListEntry> {
        self.list.iter()
    }

    pub fn to_hashmap(&self) -> HashMap<Vec<u8>, Vec<u8>> {
        self.list.iter()
            .map(|entry| (entry.quorum_hash.clone(), entry.key.clone()))
            .collect()
    }
}

impl Default for QuorumList {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<QuorumListEntry>> for QuorumList {
    fn from(list: Vec<QuorumListEntry>) -> Self {
        Self { list }
    }
}

impl FromIterator<QuorumListEntry> for QuorumList {
    fn from_iter<T: IntoIterator<Item = QuorumListEntry>>(iter: T) -> Self {
        Self {
            list: iter.into_iter().collect(),
        }
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub struct QuorumListEntry {
    #[serde(with = "hex")]
    pub quorum_hash: Vec<u8>,
    #[serde(with = "hex")]
    pub key: Vec<u8>,
    pub height: u32,
    pub members: Vec<QuorumMember>,
    pub threshold_signature: String,
    pub mining_members_count: u32,
    pub valid_members_count: u32,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
)]
pub struct QuorumMember {
    pub proTxHash: String,
    pub pubKeyOperator: String,
    pub valid: bool,
    pub isPublicKeyShare: bool,
}

impl QuorumListEntry {
    pub fn new(quorum_hash: Vec<u8>, key: Vec<u8>) -> Self {
        Self { 
            quorum_hash, 
            key,
            height: 0,
            members: Vec::new(),
            threshold_signature: String::new(),
            mining_members_count: 0,
            valid_members_count: 0,
        }
    }
    
    pub fn new_extended(
        quorum_hash: Vec<u8>, 
        key: Vec<u8>,
        height: u32,
        members: Vec<QuorumMember>,
        threshold_signature: String,
        mining_members_count: u32,
        valid_members_count: u32,
    ) -> Self {
        Self { 
            quorum_hash, 
            key,
            height,
            members,
            threshold_signature,
            mining_members_count,
            valid_members_count,
        }
    }
}