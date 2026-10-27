"""List untranslated and stale string keys per locale. Exit code 1 when any locale is incomplete.

Usage: python scripts/translation-status.py [locale ...]   e.g. ja zh-rCN
"""

from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

RES = Path(__file__).resolve().parents[1] / "app/src/main/res"
LOCALE_DIR = re.compile(r"values-([a-z]{2}(?:-r[A-Z]{2})?)$")


def keys(directory):
    found = set()
    for path in sorted(directory.glob("*.xml")):
        for element in ET.parse(path).getroot():
            if element.tag not in ("string", "plurals", "string-array"):
                continue
            if element.get("translatable") == "false":
                continue
            # Arrays that only reference other strings need no translation of their own.
            if element.tag == "string-array" and all((item.text or "").startswith("@string/") for item in element):
                continue
            found.add(element.get("name"))
    return found


def main(selected):
    default = keys(RES / "values")
    locales = sorted(m.group(1) for d in RES.iterdir() if (m := LOCALE_DIR.match(d.name)))
    incomplete = False
    for locale in selected or locales:
        have = keys(RES / f"values-{locale}")
        missing = sorted(default - have)
        stale = sorted(have - default)
        print(f"{locale}: {len(default) - len(missing)}/{len(default)} translated"
              + (f", {len(stale)} not in the default strings" if stale else ""))
        for key in missing:
            print(f"  missing {key}")
        for key in stale:
            print(f"  stale   {key}")
        incomplete |= bool(missing)
    return 1 if incomplete else 0


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    sys.exit(main(sys.argv[1:]))
