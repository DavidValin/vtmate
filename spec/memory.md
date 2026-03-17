# Memory System Overview

The **Memory** module is a lightweight knowledge‑base that stores *knowledge units* together with their embeddings.  It is used by the AI‑assistant to recall past events and facts.

## 1. Data Model

- **KnowledgeUnit** – a semantic triple consisting of `subject`, `predicate`, `object`, optional `location` and a timestamp.
- **VecKnowledgeUnit** – the unit plus a 128‑dimensional embedding vector.
- **Memory** – holds the HNSW graph (`hnsw`), a map of IDs to `VecKnowledgeUnit` (`index_map`), and three *inverted indexes* (subject, predicate, object) that map a value to the set of IDs that contain it.

## 2. Persistence

The entire state is stored in a single **sled** database file named `memory.json` (the name is a bit of a misnomer – it is a sled DB, not a plain JSON file).  The DB contains one key:

```
"memory" => <bytes>
```

The bytes are a JSON object with the following fields:

|-------------------------------------------------------------------------------------|
| Field               | Type                       | Meaning                          |
|---------------------|----------------------------|----------------------------------|
| `next_id`           | u64                        | Next available ID for a new unit |
| `units`             | map<u64, VecKnowledgeUnit> | All stored units, keyed by ID    |
| `subject_index`     | map<string, Vec<u64>>      | Inverted index for subjects      |
| `predicate_index`   | map<string, Vec<u64>>      | Inverted index for predicates    |
| `object_index`      | map<string, Vec<u64>>      | Inverted index for objects       |
---------------------------------------------------------------------------------------

When `Memory::save_to_file` is called it serialises this structure with `serde_json::to_vec` and writes it to sled.  Loading reverses the process: the bytes are read, deserialised, and the HNSW graph is rebuilt from the embeddings.

## 3. Storing a Unit

```
let id = memory.next_id;
memory.index_map.insert(id, VecKnowledgeUnit{embedding, knowledge: unit.clone()});
memory.hnsw.insert((&embedding, id));
memory.next_id += 1;
// update inverted indexes
subject_index.entry(unit.subject).or_default().push(id);
predicate_index.entry(unit.predicate.name).or_default().push(id);
object_index.entry(unit.object).or_default().push(id);
```

After the in‑memory state is updated, `save_to_file` persists the data.

## 4. Querying

### a. Full‑text / Embedding query

`Memory::query(query, k, ef_search)` performs a vector similarity search:
1. The query string is turned into a 128‑dimensional vector via `embed_text` (hash‑based word frequency).
2. HNSW returns the `k` nearest neighbour IDs.
3. The corresponding `KnowledgeUnit`s are returned.
4. A final filter keeps only results that contain any word from the query (case‑insensitive, alphanumeric only).

### b. Inverted‑index lookup

Functions `get_by_subject`, `get_by_predicate`, `get_by_object`, and `get_by_location` simply look up the relevant ID list in the inverted index and fetch the units.  This is O(1) to get the list plus O(1) per result.

## 5. CLI Commands

The binary exposes four new flags:

```
--get-memories-by-subject <subject>
--get-memories-by-predicate <predicate>
--get-memories-by-object <object>
--get-memories-by-location <location>
```

Each flag loads `memory.json`, calls the appropriate `get_by_*` method, and prints each unit’s context via `Memory::build_context_from_units`.

## 6. Summary

* Units are stored with an embedding and an ID.
* All data is persisted as a single sled database entry containing JSON.
* Retrieval can be done either by embedding similarity or by fast inverted‑index lookups.
* The CLI provides convenient commands for inspecting and querying the memory.

This design keeps the runtime lightweight while enabling efficient queries over thousands of units.

## 7. LLM integration

Store memories:

1. when sending llm prompt, provide "story_memory" tool with available predicates
2. execute tool calls (new memories to store) from LLM response

Retrieve memories:

1. Before submitting prompt, query memories
2. Collect results (knowledge units) and produce text readable sentences using build_context_from_units(&retrieved_units);
3. Provide context to the llm
