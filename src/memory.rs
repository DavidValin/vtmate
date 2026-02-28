use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::{HashMap, HashSet};
use hnsw_rs::prelude::*;
use anndists::dist::DistL2;       // L2 distance implementation
use serde::{Serialize, Deserialize};
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter};
use serde_json::Value;
use std::thread::{self, JoinHandle};
use std::time::Duration;

// Sample of LLM use
// 
// Store memories:
// ----------------------------------------------
//   Construct the prompt for llm:
//     1. provide a list of available predicates
//     2. provide sample tool call (example json at src/tools/store_memory_sample.json)
//     3. add original user prompt to tool call sample and submit it
//     4. execute tool call from LLM response
//
// Retrieve memories:
// ----------------------------------------------
//   Process the prompt before submitting to llm:
//     1. Spawn N llm requests to provide a list of memories to retrieve based on llm response and available predicates
//     2. For each memory to search, execute the search:
//
//       let query = "Who did Alice meet recently in NYC?";
//       let top_k = 5;
//       let ef_search = 50;
//       let retrieved_units = memory.query(query, top_k, ef_search);
//       let context_text = build_context_from_units(&retrieved_units);
//       println!("Context for LLM:\n{}", context_text);
//
//     3. Provide context sentences to the LLM

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Predicate {
  pub name:    String,
  pub inverse: String,
}

impl Predicate {
  pub fn to_string(&self) -> String {
    self.name.clone()
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KnowledgeUnit {
  pub subject:   String,
  pub predicate: Predicate,
  pub object:    String,
  pub location:  Option<String>,
  pub timestamp: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VecKnowledgeUnit {
  pub embedding: Vec<f32>,
  pub knowledge: KnowledgeUnit,
}



pub struct Memory {
  hnsw: Hnsw<'static, f32, DistL2>,
  index_map: HashMap<usize, VecKnowledgeUnit>,
  next_id: usize,
}

impl Memory {

  pub fn new(expected_elements: usize) -> Self {
    // HNSW parameters
    let max_nb_connection = 16;
    let max_layer = 16.min((expected_elements as f32).ln().trunc() as usize);
    let ef_construction = 200;

    let hnsw: Hnsw<'static, f32, DistL2> = 
      Hnsw::new(
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
    }
  }

  fn embed_text(text: &str) -> Vec<f32> {
    let mut vec: Vec<f32> = text.chars().map(|c| c as u32 as f32).collect();
    vec.resize(128, 0.0);
    vec
  }

  // returns context as sentences (for llm usage)
  pub fn build_context_from_units(units: &[KnowledgeUnit]) -> String {
    let mut context_phrases = Vec::new();
    for unit in units {
      let loc = unit.location.clone().unwrap_or("unknown location".into());

      // Convert SystemTime -> seconds since UNIX_EPOCH
      let duration_since_epoch = unit.timestamp.duration_since(UNIX_EPOCH)
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
        "{} {} {} in {} at {}.",
        unit.subject, unit.predicate.name, unit.object, loc, time_str
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

    self.index_map.insert(
      id,
      VecKnowledgeUnit { 
        embedding: embedding.clone(), 
        knowledge: unit 
      },
    );

    self.hnsw.insert((&embedding, id));
    self.next_id += 1;
  }


  fn filter_units<'a>(
      &'a self,
      units: impl Iterator<Item=&'a VecKnowledgeUnit>,
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
    let units = self.index_map.values()
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
    let units = self.index_map.values()
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
    let units = self.index_map.values()
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

    neighbors.into_iter()
      .map(|neigh| self.index_map[&neigh.d_id].knowledge.clone())
      .collect()
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
    let file = OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
    let writer = BufWriter::new(file);

    // Save all index_map entries + next_id
    let data = json!({
      "next_id": self.next_id,
      "units": self.index_map,
    });

    serde_json::to_writer(writer, &data)?;
    Ok(())
  }


  /// Load memory from disk, reconstructing HNSW graph
  pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let data: Value = serde_json::from_reader(reader)?;

    let units_map: HashMap<usize, VecKnowledgeUnit> = serde_json::from_value(data["units"].clone())?;
    let next_id = data["next_id"].as_u64().unwrap_or(0) as usize;
    let expected_elements = units_map.len().max(1);

    // Rebuild HNSW
    let max_nb_connection = 16;
    let max_layer = 16.min((expected_elements as f32).ln().trunc() as usize);
    let ef_construction = 200;

    let mut hnsw: Hnsw<'static, f32, DistL2> = Hnsw::new(
      max_nb_connection,
      expected_elements,
      max_layer,
      ef_construction,
      DistL2 {},
    );

    for (id, v) in &units_map {
      hnsw.insert((&v.embedding, *id));
    }

    Ok(Memory {
      hnsw,
      index_map: units_map,
      next_id,
    })
  }


  pub fn autosave(self, path: String, interval_sec: u64) -> JoinHandle<()>
    where Self: Send + 'static,
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

static AVAILABLE_PREDICATES: &[(&str, &str)] = &[
  ("believed",          "was believed by"),
  ("assumed",           "was assumed by"),
  ("made",              "was made by"),
  ("saw",               "was seen by"),
  ("said to",           "was told by"),
  ("failed at",         "was a failure of"),
  ("wanted",            "was wanted by"),
  ("thought",           "was thought of by"),
  ("asked about",       "was asked about by"),
  ("planned",           "was planned"),
  ("requested",         "was requested by"),
  ("ordered",           "was ordered by"),
  ("complained about",  "received a complaint from"),
  ("ocurred at",        "was created by"),
  ("created",           "was created by"),
  ("met with",          "was met by"),
  ("destroyed",         "was destroyed by"),
  ("modified",          "was modified by"),
  ("examined",          "was examined by"),
  ("inspected",         "was inspected by"),
  ("evaluated",         "was evaluated by"),
  ("tested",            "was tested by"),
  ("analyzed",          "was analyzed by"),
  ("calculated",        "was calculated by"),
  ("estimated",         "was estimated by"),
  ("predicted",         "was predicted by"),
  ("performed",         "was performed by"),
  ("executed",          "was executed by"),
  ("completed",         "was completed by"),
  ("succeeded",         "was succeeded by"),
  ("confirmed",         "was confirmed by"),
  ("approved",          "was approved by"),
  ("denied",            "was denied by"),
  ("received",          "was received by"),
  ("sent",              "was sent by"),
  ("delivered",         "was delivered by"),
  ("communicated",      "was communicated to"),
  ("informed",          "was informed by"),
  ("informed about",    "was informed about by"),
  ("questioned",        "was questioned by"),
  ("inquired",          "was inquired by"),
  ("participated in",   "was participated in by"),
  ("attended",          "was attended by"),
  ("presented",         "was presented by"),
  ("displayed",         "was displayed by"),
  ("demonstrated",      "was demonstrated by"),
];
