// ------------------------------------------------------------------
//  Memory
// ------------------------------------------------------------------

use anndists::dist::DistL2; // L2 distance implementation
// bincode removed
use crossbeam_channel::Sender;
use hnsw_rs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::json;
use sled;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
// File and BufReader/BufWriter removed
use crate::util;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
pub static TX_UI: OnceLock<Sender<String>> = OnceLock::new();

// API
// ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Predicate {
  pub name: String,
  pub inverse: String,
}

impl Predicate {
  pub fn to_string(&self) -> String {
    self.name.clone()
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeUnit {
  pub subject: String,
  pub predicate: Predicate,
  pub object: String,
  pub location: Option<String>,
  pub timestamp: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VecKnowledgeUnit {
  pub embedding: Vec<f32>,
  pub knowledge: KnowledgeUnit,
}

pub struct Memory {
  hnsw: Hnsw<'static, f32, DistL2>,
  pub index_map: HashMap<usize, VecKnowledgeUnit>,
  next_id: usize,
  // Inverted indexes for disk‑side search
  subject_index: HashMap<String, Vec<usize>>,
  predicate_index: HashMap<String, Vec<usize>>,
  object_index: HashMap<String, Vec<usize>>,
}

impl Memory {
  pub fn new(expected_elements: usize) -> Self {
    // HNSW parameters
    let max_nb_connection = 16;
    let max_layer = std::cmp::max(1, 16.min((expected_elements as f32).ln().trunc() as usize));
    let ef_construction = 200;

    let hnsw: Hnsw<'static, f32, DistL2> = Hnsw::new(
      max_nb_connection,
      expected_elements,
      max_layer,
      ef_construction,
      DistL2 {},
    );

    Memory {
      hnsw,
      index_map: HashMap::new(),
      next_id: 0,
      subject_index: HashMap::new(),
      predicate_index: HashMap::new(),
      object_index: HashMap::new(),
    }
  }

  fn embed_text(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; 128];
    for word in text.split_whitespace() {
      let mut hasher = std::collections::hash_map::DefaultHasher::new();
      word.hash(&mut hasher);
      let idx = (hasher.finish() as usize) % 128;
      vec[idx] += 1.0;
    }
    vec
  }

  // returns context as sentences (for llm usage)
  pub fn build_context_from_units(units: &[KnowledgeUnit]) -> String {
    let mut context_phrases = Vec::new();
    for unit in units {
      let location = unit.location.clone().unwrap_or("unknown location".into());

      // Convert SystemTime -> seconds since UNIX_EPOCH
      let duration_since_epoch = unit
        .timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

      // Optional: simple date formatting (UTC)
      let secs = duration_since_epoch;
      let days = secs / 86400;
      let hours = (secs % 86400) / 3600;
      let minutes = (secs % 3600) / 60;

      let time_str = format!("{}d {}h {}m since epoch", days, hours, minutes);

      // Build sentence
      let phrase = format!(
        "Subject: '\x1b[32m{}\x1b[0m' Predicate: '\x1b[34m{}\x1b[0m' Object: '\x1b[35m{}\x1b[0m' in '\x1b[36m{}\x1b[0m' at '\x1b[33m{}\x1b[0m'.",
        unit.subject, unit.predicate.name, unit.object, location, time_str
      );
      context_phrases.push(phrase);
    }
    context_phrases.join("\n")
  }

  pub fn store(&mut self, unit: KnowledgeUnit) {
    let text = format!(
      "{} {} {}",
      unit.subject,
      unit.predicate.to_string(),
      unit.object
    );

    let embedding = Memory::embed_text(&text);
    let id = self.next_id;

    let unit_clone = unit.clone();
    self.index_map.insert(
      id,
      VecKnowledgeUnit {
        embedding: embedding.clone(),
        knowledge: unit_clone,
      },
    );

    self.hnsw.insert((&embedding, id));
    self.next_id += 1;
    // Update inverted indexes
    self
      .subject_index
      .entry(unit.subject.clone())
      .or_default()
      .push(id);
    self
      .predicate_index
      .entry(unit.predicate.name.clone())
      .or_default()
      .push(id);
    self
      .object_index
      .entry(unit.object.clone())
      .or_default()
      .push(id);
    // Send UI notification
    if let Some(sender) = TX_UI.get() {
      // Send UI notification using a clone to avoid moving the original unit
      let _ = sender.send(format!(
        "line|🧠 Memory saved: {} {} {}",
        unit.subject.clone(),
        unit.predicate.name.clone(),
        unit.object.clone()
      ));
    }
  }

  fn filter_units<'a>(
    &'a self,
    units: impl Iterator<Item = &'a VecKnowledgeUnit>,
    location: Option<&str>,
    start: Option<SystemTime>,
    end: Option<SystemTime>,
  ) -> Vec<KnowledgeUnit> {
    units
      .filter(|v| {
        let k = &v.knowledge;
        let loc_ok = match (location, &k.location) {
          (Some(l), Some(kl)) => kl == l,
          (Some(_), None) => false,
          (None, _) => true,
        };
        let ts_ok = match (start, end) {
          (Some(s), Some(e)) => k.timestamp >= s && k.timestamp <= e,
          (Some(s), None) => k.timestamp >= s,
          (None, Some(e)) => k.timestamp <= e,
          (None, None) => true,
        };
        loc_ok && ts_ok
      })
      .map(|v| v.knowledge.clone())
      .collect()
  }

  pub fn get_by_subject(
    &self,
    subject: &str,
    location: Option<&str>,
    start: Option<SystemTime>,
    end: Option<SystemTime>,
  ) -> Vec<KnowledgeUnit> {
    let units = self
      .index_map
      .values()
      .filter(move |v| v.knowledge.subject == subject);
    self.filter_units(units, location, start, end)
  }

  pub fn get_by_predicate(
    &self,
    predicate_name: &str,
    location: Option<&str>,
    start: Option<SystemTime>,
    end: Option<SystemTime>,
  ) -> Vec<KnowledgeUnit> {
    let units = self
      .index_map
      .values()
      .filter(move |v| v.knowledge.predicate.name == predicate_name);
    self.filter_units(units, location, start, end)
  }

  pub fn get_by_object(
    &self,
    object: &str,
    location: Option<&str>,
    start: Option<SystemTime>,
    end: Option<SystemTime>,
  ) -> Vec<KnowledgeUnit> {
    let units = self
      .index_map
      .values()
      .filter(move |v| v.knowledge.object == object);
    self.filter_units(units, location, start, end)
  }

  pub fn get_by_location(
    &self,
    location: &str,
    start: Option<SystemTime>,
    end: Option<SystemTime>,
  ) -> Vec<KnowledgeUnit> {
    let units = self.index_map.values();
    self.filter_units(units, Some(location), start, end)
  }

  pub fn query(&self, query: &str, k: usize, ef_search: usize) -> Vec<KnowledgeUnit> {
    let q_embed = Memory::embed_text(query);
    let neighbors = self.hnsw.search(&q_embed, k, ef_search);

    neighbors
      .into_iter()
      .map(|neigh| self.index_map[&neigh.d_id].knowledge.clone())
      .collect()
  }

  /// Disk‑side search using the inverted indexes
  pub fn query_disk(&self, field: &str, value: &str) -> Vec<KnowledgeUnit> {
    let id_list = match field {
      "subject" => self.subject_index.get(value),
      "predicate" => self.predicate_index.get(value),
      "object" => self.object_index.get(value),
      _ => None,
    };

    match id_list {
      Some(ids) => ids
        .iter()
        .map(|&id| self.index_map[&id].knowledge.clone())
        .collect(),
      None => vec![],
    }
  }

  pub fn to_json_graph(&self) -> serde_json::Value {
    let mut nodes_set = HashSet::new();
    let mut nodes = vec![];
    let mut edges = vec![];

    for v in self.index_map.values() {
      let k = &v.knowledge;

      if nodes_set.insert(k.subject.clone()) {
        nodes.push(json!({ "id": k.subject, "label": k.subject }));
      }
      if nodes_set.insert(k.object.clone()) {
        nodes.push(json!({ "id": k.object, "label": k.object }));
      }

      let mut edge = json!({
        "from": k.subject,
        "to": k.object,
        "label": k.predicate.name,
      });

      let timestamp = k.timestamp.duration_since(UNIX_EPOCH).unwrap().as_secs();
      edge["timestamp"] = json!(timestamp);

      if let Some(loc) = &k.location {
        edge["location"] = json!(loc);
      }

      edges.push(edge);
    }

    json!({ "nodes": nodes, "edges": edges })
  }

  /// Save memory to disk (both embeddings & knowledge units)
  pub fn save_to_file(&self, path: &str) -> anyhow::Result<()> {
    // Store in sled under a single key
    let db = sled::open(path)?;
    let persist = serde_json::json!({
      "next_id": self.next_id,
      "units": self.index_map,
      "subject_index": self.subject_index,
      "predicate_index": self.predicate_index,
      "object_index": self.object_index
    });
    let bytes = serde_json::to_vec(&persist)?;
    db.insert(b"memory", bytes)?;
    db.flush()?;
    Ok(())
  }

  /// Load memory from disk, reconstructing HNSW graph
  pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
    let db = sled::open(path)?;
    let data = db
      .get(b"memory")?
      .ok_or_else(|| anyhow::anyhow!("memory data not found"))?;
    let persist: serde_json::Value = serde_json::from_slice(&data)?;

    let units_map: HashMap<usize, VecKnowledgeUnit> =
      serde_json::from_value(persist["units"].clone())?;
    let next_id = persist["next_id"].as_u64().unwrap_or(0) as usize;
    let expected_elements = units_map.len().max(1);

    let max_nb_connection = 16;
    let max_layer = std::cmp::max(1, 16.min((expected_elements as f32).ln().trunc() as usize));
    let ef_construction = 200;

    let hnsw: Hnsw<'static, f32, DistL2> = Hnsw::new(
      max_nb_connection,
      expected_elements,
      max_layer,
      ef_construction,
      DistL2 {},
    );

    for (id, v) in &units_map {
      hnsw.insert((&v.embedding, *id));
    }

    let mut memory = Memory {
      hnsw,
      index_map: units_map,
      next_id,
      subject_index: serde_json::from_value(persist["subject_index"].clone())?,
      predicate_index: serde_json::from_value(persist["predicate_index"].clone())?,
      object_index: serde_json::from_value(persist["object_index"].clone())?,
    };

    Ok(memory)
  }

  pub fn autosave(self, path: String, interval_sec: u64) -> JoinHandle<()>
  where
    Self: Send + 'static,
  {
    thread::spawn(move || {
      loop {
        if let Err(e) = self.save_to_file(&path) {
          eprintln!("Failed to autosave memory: {:?}", e);
        }
        thread::sleep(Duration::from_secs(interval_sec));
      }
    })
  }
}

// PRIVATE
// ------------------------------------------------------------------

pub fn ensure_memory_path() -> String {
  let home = util::get_user_home_path().expect("Failed to get home dir");
  let dir = home.join(".ai-mate/agents/default");
  std::fs::create_dir_all(&dir).expect("Failed to create memory directory");
  dir.join("memory").to_string_lossy().into_owned()
}

use std::path::Path;

pub fn ensure_memory_file(path: &str) -> anyhow::Result<Memory> {
  if Path::new(path).exists() {
    Memory::load_from_file(path)
  } else {
    Ok(Memory::new(1000))
  }
}

static AVAILABLE_PREDICATES: &[(&str, &str)] = &[
  // predicate          // inverse
  ("believed", "was believed by"),
  ("assumed", "was assumed by"),
  ("made", "was made by"),
  ("saw", "was seen by"),
  ("said", "was said by"),
  ("said to", "was told by"),
  ("failed at", "was a failure of"),
  ("wanted", "was wanted by"),
  ("thought", "was thought of by"),
  ("asked about", "was asked about by"),
  ("planned", "was planned"),
  ("requested", "was requested by"),
  ("ordered", "was ordered by"),
  ("complained about", "received a complaint from"),
  ("ocurred at", "was created by"),
  ("created", "was created by"),
  ("met with", "was met by"),
  ("destroyed", "was destroyed by"),
  ("modified", "was modified by"),
  ("examined", "was examined by"),
  ("inspected", "was inspected by"),
  ("evaluated", "was evaluated by"),
  ("tested", "was tested by"),
  ("analyzed", "was analyzed by"),
  ("calculated", "was calculated by"),
  ("estimated", "was estimated by"),
  ("predicted", "was predicted by"),
  ("performed", "was performed by"),
  ("executed", "was executed by"),
  ("completed", "was completed by"),
  ("succeeded", "was succeeded by"),
  ("confirmed", "was confirmed by"),
  ("approved", "was approved by"),
  ("denied", "was denied by"),
  ("received", "was received by"),
  ("sent", "was sent by"),
  ("delivered", "was delivered by"),
  ("communicated", "was communicated to"),
  ("informed", "was informed by"),
  ("informed about", "was informed about by"),
  ("questioned", "was questioned by"),
  ("inquired", "was inquired by"),
  ("participated in", "was participated in by"),
  ("attended", "was attended by"),
  ("presented", "was presented by"),
  ("displayed", "was displayed by"),
  ("demonstrated", "was demonstrated by"),
];
