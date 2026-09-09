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

# Landscape images get capped in height so they slot into the text flow instead
# of being bumped to the next page; portrait ones keep the full text width and
# take a page of their own.
python3 - "$tmp/body.html" "$srcdir" <<'PYIMG'
import os, re, struct, sys, urllib.parse

html_path, srcdir = sys.argv[1], sys.argv[2]

def size(path):
    with open(path, 'rb') as f:
        head = f.read(32)
        if head[:8] == b'\x89PNG\r\n\x1a\n':
            return struct.unpack('>II', head[16:24])
        if head[:6] in (b'GIF87a', b'GIF89a'):
            return struct.unpack('<HH', head[6:10])
        if head[:2] == b'\xff\xd8':
            f.seek(2)
            while True:
                b = f.read(1)
                if not b:
                    return None
                if b != b'\xff':
                    continue
                marker = f.read(1)
                if marker in (b'\xc0', b'\xc1', b'\xc2', b'\xc3'):
                    f.read(3)
                    h, w = struct.unpack('>HH', f.read(4))
                    return w, h
                (length,) = struct.unpack('>H', f.read(2))
                f.seek(length - 2, os.SEEK_CUR)
    return None

def is_wide(src):
    if re.match(r'[a-z][a-z0-9+.-]*:', src):   # remote: not measurable, leave as is
        return False
    path = os.path.join(srcdir, urllib.parse.unquote(src.split('#')[0].split('?')[0]))
    try:
        dims = size(path)
    except OSError:
        return False
    return bool(dims) and dims[1] > 0 and dims[0] >= 1.2 * dims[1]

def tag(m):
    src = re.search(r'src="([^"]*)"', m.group(0))
    if src and is_wide(src.group(1)):
        return m.group(0).replace('<img', '<img class="wide"', 1)
    return m.group(0)

html = open(html_path).read()
open(html_path, 'w').write(re.sub(r'<img\b[^>]*>', tag, html))
PYIMG

{
  cat <<HEAD
<!doctype html>
<meta charset="utf-8">
<base href="file://$srcdir/">
<title>$(basename "${in%.*}")</title>
<style>
  @page { size: A4; margin: 18mm 16mm; }
  * { -webkit-print-color-adjust: exact; print-color-adjust: exact; }
  body { font: 11pt/1.55 -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
         color: #1f2328; margin: 0; }
  h1, h2, h3, h4 { line-height: 1.25; margin: 1.4em 0 .5em; break-after: avoid; }
  h1 { font-size: 1.9em; border-bottom: 1px solid #d1d9e0; padding-bottom: .3em; }
  h2 { font-size: 1.45em; border-bottom: 1px solid #d1d9e0; padding-bottom: .3em; }
  img, svg, video { max-width: 100%; max-height: 250mm; height: auto; break-inside: avoid; }
  img.wide { max-height: 68mm; }
  p > img:only-child { display: block; margin: 1em auto; }
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
