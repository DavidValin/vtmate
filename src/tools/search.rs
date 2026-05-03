use super::Tool;
use crate::tools::web_fetch::WebFetchTool;
use serde_json::{Value, json};
use urlencoding::encode;

// API
// ------------------------------------------------------------------

pub struct SearchTool;

impl SearchTool {
  pub fn new() -> Self {
    SearchTool
  }
}

impl Tool for SearchTool {
  fn name(&self) -> &str {
    "search"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let query = match tool_call_args.get("q") {
      Some(v) if v.is_string() => v.as_str().unwrap().to_string(),
      Some(v) if v.is_object() => v
        .get("value")
        .and_then(|vv| vv.as_str())
        .unwrap_or("")
        .to_string(),
      _ => return Err("Missing 'q' parameter".into()),
    };
    let encoded = encode(&query);
    let url = format!("https://search.brave.com/search?q={}", encoded);
    // Use web_fetch internally
    let web_args = json!({
      "url": url,
      "content_type": "text"
    });
    let wf = WebFetchTool::new();
    let wf_output = wf.handle(&web_args)?;
    // Parse the JSON response from web_fetch and format links
    let parsed_res = serde_json::from_str::<Value>(&wf_output);
    let result = match parsed_res {
      Ok(parsed) => {
        let mut res = String::from("Here are some result urls for the search:\n\n");
        if let Some(links) = parsed.get("links").and_then(|v| v.as_array()) {
          let mut count = 0;
          let mut seen = std::collections::HashSet::new();
          for link in links {
            let href = link.get("href").and_then(|v| v.as_str()).unwrap_or("");
            // Skip links without a base URL (relative paths only)
            if !href.starts_with("http://") && !href.starts_with("https://") {
              continue;
            }
            let text = link.get("text").and_then(|v| v.as_str()).unwrap_or("");
            // Skip links with no anchor text or duplicate href
            if text.is_empty() || !seen.insert(href) {
              continue;
            }
            res.push_str(" * ");
            res.push_str(href);
            res.push_str("  :  ");
            res.push_str(text);
            res.push_str("\n");
            count += 1;
            if count >= 10 {
              break;
            }
          }
          res.push_str("\nYou can use 'web_fetch' tool to inspect any of those urls.\n\n");
        }
        Ok(res)
      }
      Err(_) => Ok(wf_output),
    };
    return result;
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "search",
        "description": "Searches the web for a term (q) and return a list of url results. Use this tool to find urls you need to retrieve external information.",
        "parameters": {
          "type": "object",
          "properties": {
            "q": {
              "type": "string"
            }
          },
          "required": ["q"]
        }
      }
    }))
  }
}
