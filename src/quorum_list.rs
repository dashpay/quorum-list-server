use serde::{Deserialize, Serialize};
use bincode::{Decode, Encode};
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Encode,
    Decode,
)]
pub struct QuorumList {
    pub list: Vec<QuorumListEntry>,
}

impl From<a> for QuorumList {
    fn from(value: a) -> Self {
        todo!()
    }
}
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Encode,
    Decode,
)]
pub struct QuorumListEntry {
    pub quorum_hash: [u8;32],
    pub key: [u8;48],
}