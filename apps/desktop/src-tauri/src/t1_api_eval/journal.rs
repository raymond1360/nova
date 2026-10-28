use super::{hash, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub identity: String,
    pub pre_sha256: String,
    pub post_sha256: String,
    pub bytes: usize,
    pub state: String,
    pub durability_limit: String,
}
pub struct Transaction {
    pub(crate) root: PathBuf,
    pub(crate) pre: Vec<u8>,
    pub(crate) post: Vec<u8>,
}
impl Transaction {
    /// Historical journal state, not proof that the target has not changed since receipt.
    pub fn status(&self) -> Result<&'static str> {
        let receipt_path = self.root.join("receipt.json");
        if receipt_path.exists() {
            let r: Receipt =
                serde_json::from_slice(&fs::read(receipt_path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("UNCERTAIN invalid receipt: {e}"))?;
            let intent: serde_json::Value = serde_json::from_slice(
                &fs::read(self.root.join("mutation-intent.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if r.state != "COMMITTED"
                || r.pre_sha256 != hash(&self.pre)
                || r.post_sha256 != hash(&self.post)
                || r.bytes != self.post.len()
                || intent["identity"] != r.identity
            {
                return Err("UNCERTAIN inconsistent receipt".into());
            }
            if self.root.join("recovered.json").exists() {
                return Ok("RECOVERED_HISTORICAL");
            }
            return Ok("COMMITTED_HISTORICAL");
        }
        if self.root.join("mutation-intent.json").exists() {
            return Ok("UNCERTAIN");
        }
        Ok("PREPARED")
    }
    pub fn prepare(root: &Path, pre: &[u8], post: &[u8]) -> Result<Self> {
        fs::create_dir(root).map_err(|e| e.to_string())?;
        write_new(&root.join("preimage.bin"), pre)?;
        write_new(&root.join("postimage.bin"), post)?;
        write_new(
            &root.join("prepared.json"),
            &serde_json::to_vec(
                &serde_json::json!({"pre":hash(pre),"post":hash(post),"state":"PREPARED"}),
            )
            .unwrap(),
        )?;
        Ok(Self {
            root: root.into(),
            pre: pre.into(),
            post: post.into(),
        })
    }
    pub(crate) fn intent(&self, identity: &str) -> Result<()> {
        write_new(
            &self.root.join("mutation-intent.json"),
            &serde_json::to_vec(
                &serde_json::json!({"identity":identity,"state":"UNCERTAIN_UNTIL_RECEIPT"}),
            )
            .unwrap(),
        )
    }
    pub(crate) fn receipt(&self, r: &Receipt) -> Result<()> {
        write_new(
            &self.root.join("receipt.json"),
            &serde_json::to_vec_pretty(r).unwrap(),
        )
    }
    /// Reopen evidence only; strict hash validation prevents treating partial journal as committed.
    pub fn reopen(root: &Path) -> Result<Self> {
        let pre = fs::read(root.join("preimage.bin")).map_err(|e| e.to_string())?;
        let post = fs::read(root.join("postimage.bin")).map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join("prepared.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if v["pre"] != hash(&pre) || v["post"] != hash(&post) {
            return Err("corrupt journal".into());
        }
        Ok(Self {
            root: root.into(),
            pre,
            post,
        })
    }
}
