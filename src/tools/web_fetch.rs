// ------------------------------------------------------------------
//  Tool: Web Fetch
// ------------------------------------------------------------------

use super::Tool;

use regex::Regex;
use serde_json::Value;
use serde_json::json;

fn strip_html(html: &str) -> String {
  // Remove script and style tags and their content
  let script_re = Regex::new(r"(?s)<script[^>]*>.*?</script>").unwrap();
  let no_script = script_re.replace_all(html, "");
  let style_re = Regex::new(r"(?s)<style[^>]*>.*?</style>").unwrap();
  let no_style = style_re.replace_all(&no_script, "");
  // Remove all remaining HTML tags and collapse whitespace
  // Remove all HTML tags and collapse whitespace
  let re = Regex::new(r"<[^>]*>").unwrap();
  let no_tags = re.replace_all(&no_style, "");
  // Replace multiple whitespace with single space and trim
  let space_re = Regex::new(r"\s+").unwrap();
  space_re
    .replace_all(&no_tags, " ")
    .to_string()
    .trim()
    .to_string()
}

fn extract_paragraphs(body: &str) -> Vec<String> {
  let para_re = Regex::new(r"<p\b[^>]*>(.*?)</p>").unwrap();
  para_re
    .captures_iter(body)
    .map(|cap| {
      let raw = cap.get(1).map_or("", |m| m.as_str()).to_string();
      strip_html(&raw)
    })
    .collect()
}

// API
// ------------------------------------------------------------------

pub struct WebFetchTool;

// PRIVATE
// ------------------------------------------------------------------

impl WebFetchTool {
  pub fn new() -> Self {
    WebFetchTool
  }
}

impl Tool for WebFetchTool {
  fn name(&self) -> &str {
    "web_fetch"
  }

  fn handle(
    &self,
    tool_call_args: &Value,
  ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let url = tool_call_args
      .get("url")
      .and_then(|v| v.as_str())
      .unwrap_or("");
    if url.is_empty() {
      return Ok("No URL provided".to_string());
    }
    let _content_type = tool_call_args
      .get("content_type")
      .and_then(|v| v.as_str())
      .unwrap_or("text");

    let client = reqwest::blocking::Client::builder()
      .user_agent(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:109.0) Gecko/20100101 Firefox/109.0",
      )
      .build()?;
    let response = client.get(url).send();

    match response {
      Ok(resp) => {
        if !resp.status().is_success() {
          return Ok(
            serde_json::json!({
              "status": "failed",
              "reasons": [format!(
                "HTTP {} - URL '{}' returned status {}",
                resp.status(),
                url,
                resp.status()
              )]
            })
            .to_string(),
          );
        }
        let body = resp.text()?;
        // Extract title
        let title_re = Regex::new(r#"<title>(.*?)</title>"#).unwrap();
        let title = title_re
          .captures(&body)
          .and_then(|c| c.get(1))
          .map(|m| m.as_str().to_string());
        // Extract headings and sections
        let heading_re = Regex::new(r#"<h([1-6])>(.*?)</h[1-6]>"#).unwrap();
        let mut sections = Vec::new();
        let mut headings: Vec<(usize, String)> = Vec::new();
        for cap in heading_re.captures_iter(&body) {
          let start = cap.get(0).map_or(0, |m| m.start());
          let heading_text = cap
            .get(2)
            .map_or("".to_string(), |m| m.as_str().to_string());
          if heading_text.is_empty() {
            continue;
          }
          headings.push((start, heading_text));
        }
        for i in 0..headings.len() {
          let (start, ref heading_text) = headings[i];
          let end = if i + 1 < headings.len() {
            headings[i + 1].0
          } else {
            body.len()
          };
          let raw_content = body[start..end].trim();
          let content = strip_html(raw_content);
          sections.push(serde_json::json!({"heading": heading_text, "content": content}));
        }
        // Extract links
        let link_re = Regex::new(r#"<a\s+(?:[^>]*?\s+)?href=\"([^\"]*)\"[^>]*>(.*?)</a>"#).unwrap();
        let mut links = Vec::new();
        for cap in link_re.captures_iter(&body) {
          let href = cap
            .get(1)
            .map_or("".to_string(), |m| m.as_str().to_string());
          let link_text_raw = cap
            .get(2)
            .map_or("".to_string(), |m| m.as_str().to_string());
          let link_text = strip_html(&link_text_raw);

          if href.is_empty() {
            continue;
          }

          links.push(serde_json::json!({"href": href, "text": link_text.clone()}));
        }
        let paragraphs = extract_paragraphs(&body);
        let json_body = serde_json::json!({
          "title": title,
          "paragraphs": paragraphs,
          "sections": sections,
          "links": links
        });
        Ok(json_body.to_string())
      }
      Err(e) => Ok(format!("Error fetching URL: {}. Try different url", e)),
    }
  }

  fn json_schema(&self) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(json!({
      "type": "function",
      "function": {
        "name": "web_fetch",
        "description": "Fetches a single web page using a url and returns a JSON containing its content and links",
        "parameters": {
          "type": "object",
          "properties": {
            "url": {
              "type": "string"
            },
            "content_type": {
              "type": "string",
              "enum": ["text", "html"]
            }
          },
          "required": [
            "url"
          ]
        }
      }
    }))
  }
}
