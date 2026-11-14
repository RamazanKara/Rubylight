#!/usr/bin/env python3
"""Build the GitHub Pages site into _site/.

The landing page is site/index.html. The documentation pages are rendered
from the repository's own Markdown, so they always match docs/ and rust/.
Links between rendered pages stay on the site; links to source files go to
GitHub. The download link and version come from README.md's "Download"
link, which every release updates.

The build fails on a broken link in a rendered page, README.md or the
landing page: a file that doesn't exist, or a #fragment that names no
heading (by GitHub's rules) or id in its target.

    python3 -m pip install -r site/requirements.txt
    python3 site/build.py          # writes _site/
"""

import html
import posixpath
import re
import shutil
import sys
from pathlib import Path
from urllib.parse import unquote

from markdown_it import MarkdownIt
from mdit_py_plugins.anchors import anchors_plugin

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
OUT = ROOT / "_site"
REPO = "https://github.com/RamazanKara/Rubylight"
SITE_URL = "https://ramazankara.github.io/Rubylight/"

# Internal planning and design notes: in the repository, not on the site.
# Links to them from published pages go to GitHub.
INTERNAL = {
    "docs/rubylight-3.0-plan.md",
    "docs/couch-menu-design.md",
    "docs/encoder-focus-design.md",
    "docs/bug-hunt-2026-10-28.md",
    "docs/stability-audit-2026-10-18.md",
    "docs/pyrowave-d3d12-audit-2026-10-18.md",
    "docs/webrtc-design.md",
    "docs/wake-and-audio-design.md",
}
# Markdown rendered as pages, repository paths.
PAGES = sorted(
    [
        src
        for src in (p.relative_to(ROOT).as_posix() for p in (ROOT / "docs").glob("*.md"))
        if src not in INTERNAL
    ]
    + [
        "rust/PERFORMANCE.md",
        "rust/PERFORMANCE_WORK.md",
        "rust/README.md",
        "rust/RELEASE_NOTES.md",
        "rust/THIRD_PARTY.md",
    ]
)
# Files copied as they are when a page or the landing page links them.
MEDIA = {".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".mp4"}
STATIC = ["style.css", "favicon.svg", "poster.jpg"]
LANDING_MEDIA = ["docs/media/demo.mp4"]
# The documentation pages' top bar.
NAV = [
    ("Docs", "docs/index.html"),
    ("Get started", "docs/getting-started.html"),
    ("Configuration", "docs/configuration.html"),
    ("Troubleshooting", "docs/troubleshooting.html"),
    ("Features", "docs/features.html"),
    ("Performance", "docs/performance.html"),
    ("Release notes", "rust/RELEASE_NOTES.html"),
]


def out_path(src):
    """Site path of a rendered repository Markdown file; README.md is the landing page."""
    head, name = posixpath.split(src)
    name = "index.html" if name == "README.md" else name[:-3] + ".html"
    return posixpath.join(head, name)


def rel(target, page):
    """Relative link from one site path to another."""
    return posixpath.relpath(target, posixpath.dirname(page) or ".")


copied = set()


def copy_media(src):
    if src not in copied:
        dest = OUT / src
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / src, dest)
        copied.add(src)


broken = []
SCHEME = re.compile(r"[a-zA-Z][a-zA-Z0-9+.-]*:")
# Raw HTML in Markdown and the landing page: the ids a #fragment can name,
# and the attributes that link.
ID_ATTR = re.compile(r"""\b(?:id|name)\s*=\s*["']([^"']+)["']""")
HTML_LINK = re.compile(r"""\b(?:href|src|poster)\s*=\s*["']([^"']+)["']""")


def github_slug(title):
    """A heading's id as GitHub makes it: lower case, every character but
    letters, digits, underscores, hyphens and spaces dropped, spaces as hyphens.
    The anchors plugin adds -1, -2 to repeats, as GitHub does."""
    return re.sub(r"[^\w\- ]", "", title.strip().lower()).replace(" ", "-")


def heading_fragments(text):
    """The fragments a Markdown text answers to: its headings' ids and the
    id and name attributes of its raw HTML (code blocks don't count)."""
    found = set()
    for token in parser().parse(text):
        if token.type == "heading_open":
            found.add(token.attrGet("id"))
        elif token.type == "html_block":
            found.update(ID_ATTR.findall(token.content))
        for child in token.children or []:
            if child.type == "html_inline":
                found.update(ID_ATTR.findall(child.content))
    return found


fragment_cache = {}


def fragment_missing(target, fragment):
    """Whether a #fragment names nothing in a repository Markdown or HTML file.
    GitHub's line anchors (#L12) and other file types are not checked."""
    fragment = unquote(fragment)
    if not fragment or re.fullmatch(r"L\d+(-L\d+)?", fragment):
        return False
    if target not in fragment_cache:
        path = ROOT / target
        if not target.endswith((".md", ".html")) or not path.is_file():
            return False
        text = path.read_text(encoding="utf-8")
        fragment_cache[target] = (
            heading_fragments(text) if target.endswith(".md") else set(ID_ATTR.findall(text))
        )
    found = fragment_cache[target]
    return fragment not in found and fragment.lower() not in found


def check_fragment(src, url, target, fragment):
    if fragment_missing(target, fragment):
        broken.append(f"{src}: {url} (no such heading or id)")


def rewrite(url, src):
    """Point a link in repository Markdown at the site or at GitHub."""
    if url and url.startswith("#"):
        check_fragment(src, url, src, url[1:])
        return url
    if not url or url.startswith("//") or SCHEME.match(url):
        return url
    path, hash_, fragment = url.partition("#")
    target = posixpath.normpath(posixpath.join(posixpath.dirname(src), path))
    page = out_path(src)
    if target == "README.md" or target in PAGES:
        check_fragment(src, url, target, fragment)
        return rel(out_path(target), page) + hash_ + fragment
    if not (ROOT / target).exists() or target.startswith(".."):
        broken.append(f"{src}: {url}")
        return url
    check_fragment(src, url, target, fragment)
    if (ROOT / target).is_file() and posixpath.splitext(target)[1].lower() in MEDIA:
        copy_media(target)
        return rel(target, page) + hash_ + fragment
    kind = "tree" if (ROOT / target).is_dir() else "blob"
    return f"{REPO}/{kind}/main/{target}" + hash_ + fragment


def site_source(path):
    """The repository file served at a path of the site, or None."""
    path = path or "index.html"
    if path == "index.html":
        return "site/index.html"
    if path in STATIC:
        return f"site/{path}"
    for src in PAGES:
        if out_path(src) == path:
            return src
    if (ROOT / path).is_file() and posixpath.splitext(path)[1].lower() in MEDIA:
        return path
    return None


def check_site_link(src, url, path, copy=False):
    """A link to a path on the site; copy=True copies linked media into it."""
    path, _, fragment = unquote(path).partition("#")
    target = site_source(path.split("?")[0])
    if target is None:
        broken.append(f"{src}: {url}")
        return
    if copy and target == path and posixpath.splitext(target)[1].lower() in MEDIA:
        copy_media(target)
    check_fragment(src, url, target, fragment)


def check_repo_link(src, url):
    """A link in repository Markdown that the site doesn't render (README.md)."""
    if url.startswith(SITE_URL):
        check_site_link(src, url, url[len(SITE_URL):])
        return
    if not url or url.startswith("//") or SCHEME.match(url):
        return
    path, _, fragment = url.partition("#")
    target = posixpath.normpath(posixpath.join(posixpath.dirname(src), unquote(path))) if path else src
    if target.startswith("..") or not (ROOT / target).exists():
        broken.append(f"{src}: {url}")
        return
    check_fragment(src, url, target, fragment)


def check_readme():
    """README.md's Markdown links and the links in its raw HTML."""
    src = "README.md"
    for token in parser().parse((ROOT / src).read_text(encoding="utf-8")):
        if token.type == "html_block":
            for url in HTML_LINK.findall(token.content):
                check_repo_link(src, html.unescape(url))
        for child in token.children or []:
            if child.type == "link_open":
                check_repo_link(src, child.attrGet("href"))
            elif child.type == "image":
                check_repo_link(src, child.attrGet("src"))
            elif child.type == "html_inline":
                for url in HTML_LINK.findall(child.content):
                    check_repo_link(src, html.unescape(url))


def check_landing():
    """The landing page's links: site paths, its own #ids and our site's URLs.
    Media it links are copied into the site."""
    src = "site/index.html"
    for url in HTML_LINK.findall((ROOT / src).read_text(encoding="utf-8")):
        url = html.unescape(url)
        if url.startswith("{{"):
            continue
        if url.startswith(SITE_URL):
            check_site_link(src, url, url[len(SITE_URL):], copy=True)
        elif not url.startswith("//") and not SCHEME.match(url):
            check_site_link(src, url, url, copy=True)


def self_test():
    """The link check's own rules on a known sample, so that a change to them
    cannot quietly let every link pass."""
    assert github_slug("October 26: 3.0.0-rc.2 against the published numbers") == (
        "october-26-300-rc2-against-the-published-numbers"
    )
    assert github_slug("What's new") == "whats-new"
    assert github_slug("App × device overrides") == "app--device-overrides"
    sample = (
        "# Title\n\n## Same\n\n## Same\n\n## `code` and *emphasis*\n\n"
        '<a id="kept"></a>\n\n```\n## not a heading\n<a id="code"></a>\n```\n'
    )
    found = heading_fragments(sample)
    assert found == {"title", "same", "same-1", "code-and-emphasis", "kept"}, found
    fragment_cache["self-test.md"] = found
    assert not fragment_missing("self-test.md", "same-1")
    assert not fragment_missing("self-test.md", "L12")
    assert fragment_missing("self-test.md", "same-2")
    assert fragment_missing("self-test.md", "not-a-heading")
    del fragment_cache["self-test.md"]


_parser = None


def parser():
    global _parser
    if _parser is None:
        _parser = markdown()
    return _parser


def markdown():
    md = MarkdownIt("commonmark", {"html": True, "typographer": False})
    md.enable(["table", "strikethrough"])
    # GitHub's heading ids, so links like performance.md#next-to-vibepollo-20 work.
    md.use(
        anchors_plugin,
        min_level=1,
        max_level=6,
        slug_func=github_slug,
        permalink=True,
        permalinkSymbol="#",
    )
    md.add_render_rule("table_open", lambda *a: '<div class="table-wrap"><table>\n')
    md.add_render_rule("table_close", lambda *a: "</table></div>\n")
    return md


def render(md, src):
    tokens = md.parse((ROOT / src).read_text(encoding="utf-8"))
    title, toc = None, []
    for i, token in enumerate(tokens):
        if token.type == "heading_open":
            text = "".join(
                c.content for c in tokens[i + 1].children if c.type in ("text", "code_inline")
            ).strip()
            if token.tag == "h1" and title is None:
                title = text
            elif token.tag == "h2":
                toc.append((token.attrGet("id"), text))
        if token.type == "inline":
            for child in token.children or []:
                if child.type == "link_open":
                    child.attrSet("href", rewrite(child.attrGet("href"), src))
                elif child.type == "image":
                    child.attrSet("src", rewrite(child.attrGet("src"), src))
    mark_long_columns(tokens)
    body = md.renderer.render(tokens, md.options, {})
    return title or src, toc, body


def mark_long_columns(tokens):
    """Give columns of long prose a minimum width, so phones scroll the table
    instead of squeezing that column to a few words a line."""
    cells = []
    for i, token in enumerate(tokens):
        if token.type == "table_open":
            cells = []
        elif token.type == "tr_open":
            column = 0
        elif token.type in ("th_open", "td_open"):
            cells.append((column, token, len(tokens[i + 1].content)))
            column += 1
        elif token.type == "table_close":
            longest = {}
            for column, _, length in cells:
                longest[column] = max(longest.get(column, 0), length)
            for column, cell, _ in cells:
                if longest[column] > 160:
                    cell.attrSet("class", "long")


def fill(template, values):
    return re.sub(r"\{\{(\w+)\}\}", lambda m: values[m.group(1)], template)


def release():
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    match = re.search(r"\[Download ([^\]]+)\]\((" + re.escape(REPO) + r"/releases/tag/([^)\s]+))\)", readme)
    if not match:
        sys.exit("README.md has no [Download …](…/releases/tag/…) link")
    label, url, version = match.groups()
    return {"download_label": label, "download_url": url, "version": version}


def main():
    self_test()
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir()
    values = release()
    for name in STATIC:
        shutil.copy2(SITE / name, OUT / name)
    for src in LANDING_MEDIA:
        copy_media(src)
    (OUT / "index.html").write_text(
        fill((SITE / "index.html").read_text(encoding="utf-8"), values), encoding="utf-8"
    )

    md = parser()
    template = (SITE / "page.html").read_text(encoding="utf-8")
    for src in PAGES:
        title, toc, body = render(md, src)
        page = out_path(src)
        root = "../" * page.count("/")
        nav = "".join(
            f'<a href="{root}{href}"{" aria-current=page" if href == page else ""}>{label}</a>'
            for label, href in NAV
        )
        contents = ""
        if len(toc) >= 5:
            items = "".join(f'<li><a href="#{id_}">{html.escape(text)}</a></li>' for id_, text in toc)
            contents = f'<details class="toc"><summary>On this page</summary><ol>{items}</ol></details>'
        dest = OUT / page
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(
            fill(
                template,
                {
                    **values,
                    "title": html.escape(title),
                    "root": root,
                    "nav": nav,
                    "toc": contents,
                    "content": body,
                    "source": f"{REPO}/blob/main/{src}",
                },
            ),
            encoding="utf-8",
        )

    check_readme()
    check_landing()
    if broken:
        print("Broken links:\n  " + "\n  ".join(broken), file=sys.stderr)
        sys.exit(1)
    print(f"Built {len(PAGES)} pages and the landing page into {OUT.relative_to(ROOT)}/")


if __name__ == "__main__":
    main()
