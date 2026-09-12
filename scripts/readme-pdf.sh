#!/usr/bin/env bash
# Render a Markdown file (default README.md) to PDF, images included.
#
# Markdown -> HTML with `marked` (fetched via npx), HTML -> PDF with headless
# Chrome. GitHub raw/blob links that point back into this repo are rewritten to
# the local file, so the diagrams render without network access.
#
#   ./scripts/readme-pdf.sh [input.md] [output.pdf]
set -uo pipefail

in=${1:-README.md}
out=${2:-${in%.*}.pdf}
[[ -f $in ]] || { echo "no such file: $in" >&2; exit 2; }

command -v npx >/dev/null || { echo "npx not found (install node)" >&2; exit 2; }
chrome=$(command -v google-chrome-stable || command -v google-chrome || \
         command -v chromium || command -v chromium-browser)
[[ -n $chrome ]] || { echo "no chrome/chromium found" >&2; exit 2; }

srcdir=$(cd "$(dirname "$in")" && pwd)
tmp=$(mktemp -d) || exit 2
trap 'rm -rf "$tmp"' EXIT

# Point github raw/blob URLs at the working tree when the file exists locally.
sed -E 's#https://github\.com/[^/]+/[^/)"]+/(raw|blob)/[^/)"]+/#./#g' "$in" > "$tmp/in.md"

npx -y marked@18 --gfm -i "$tmp/in.md" -o "$tmp/body.html" || {
  echo "marked failed" >&2; exit 1; }

# Every image is stretched to the full page width (see the `img` rule below);
# the one exception is special-cased below.
python3 - "$tmp/body.html" <<'PYPOST'
import re
import sys

html_path = sys.argv[1]
html = open(html_path).read()

# marked does not add heading ids on its own, so the Index's #anchor links
# have nothing to jump to. Slug them the same way GitHub does (lowercase,
# drop anything but letters/digits/spaces/hyphens, spaces -> hyphens), and
# number apart any two headings that land on the same slug.
seen = {}

def slugify(text):
    text = re.sub(r'<[^>]+>', '', text).strip().lower()
    text = re.sub(r'[^\w\s-]', '', text)
    text = re.sub(r'[\s_]+', '-', text)
    return text

# Headings that also need a fresh page of their own, keyed by their exact
# text: "How it works"'s diagram and the steps below it are meant to read as
# one unit, so "LLM integration" right after it is pushed to its own page
# too rather than trailing into whatever room is left; "How to use it"
# should likewise not trail whatever came before it.
PAGE_START = {'How it works', 'LLM integration', 'How to use it'}

def heading(m):
    tag, text = m.group(1), m.group(2)
    slug = slugify(text)
    if slug:
        n = seen.get(slug, 0)
        seen[slug] = n + 1
        if n:
            slug = f'{slug}-{n}'
    classes = 'class="page-start" ' if text.strip() in PAGE_START else ''
    id_attr = f'id="{slug}" ' if slug else ''
    return f'<{tag} {classes}{id_attr}>{text}</{tag}>'

html = re.sub(r'<(h[1-6])>(.*?)</\1>', heading, html)

# The "How it works" diagram is shrunk (instead of the default full width)
# so it and the steps below it both fit their page together, centered.
html = html.replace(
    '<img src="./docs/en/diagrams/how-it-works.png" alt="how it works">',
    '<img src="./docs/en/diagrams/how-it-works.png" alt="how it works" class="page-fit">',
    1)
open(html_path, 'w').write(html)
PYPOST

{
  cat <<HEAD
<!doctype html>
<meta charset="utf-8">
<base href="file://$srcdir/">
<title>$(basename "${in%.*}")</title>
<style>
  @page { size: A4; margin: 18mm 16mm; }
  * { -webkit-print-color-adjust: exact; print-color-adjust: exact; }
  /* Everything else is sized in em, so this one value scales the document. */
  body { font: 9.5pt/1.55 -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
         color: #1f2328; margin: 0; }
  h1, h2, h3, h4 { line-height: 1.25; margin: 1.4em 0 .5em; break-after: avoid; }
  h1 { font-size: 1.9em; border-bottom: 1px solid #d1d9e0; padding-bottom: .3em; }
  h2 { font-size: 1.45em; border-bottom: 1px solid #d1d9e0; padding-bottom: .3em; }
  .page-start { break-before: page; }
  img, svg, video { width: 100%; height: auto; display: block; margin: 1em auto; break-inside: avoid; }
  img.page-fit { width: auto; max-width: 100%; max-height: 170mm; margin: 1em auto; }
  pre { background: #f6f8fa; border-radius: 6px; padding: 12px; overflow: visible;
        white-space: pre-wrap; word-wrap: break-word; break-inside: avoid; }
  code { font: .85em/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
  :not(pre) > code { background: #f0f1f3; border-radius: 4px; padding: .15em .35em; }
  table { border-collapse: collapse; width: 100%; break-inside: avoid; }
  th, td { border: 1px solid #d1d9e0; padding: 6px 10px; text-align: left; }
  thead tr { background: #f6f8fa; }
  blockquote { margin: 0 0 1em; padding: 0 1em; color: #59636e; border-left: .25em solid #d1d9e0; }
  a { color: #0969da; text-decoration: none; }
  hr { border: 0; border-top: 1px solid #d1d9e0; }
</style>
HEAD
  cat "$tmp/body.html"
} > "$tmp/page.html"

"$chrome" --headless --disable-gpu --no-sandbox \
  --user-data-dir="$tmp/profile" --virtual-time-budget=20000 \
  --no-pdf-header-footer --print-to-pdf="$tmp/out.pdf" \
  "file://$tmp/page.html" 2>"$tmp/chrome.log" || {
    echo "chrome failed:" >&2; tail -5 "$tmp/chrome.log" >&2; exit 1; }

[[ -s $tmp/out.pdf ]] || { echo "no pdf produced" >&2; tail -5 "$tmp/chrome.log" >&2; exit 1; }
mv "$tmp/out.pdf" "$out"
echo "wrote $out ($(du -h "$out" | cut -f1))"
