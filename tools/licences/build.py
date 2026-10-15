"""Writes plugins/about/licences.md: QuireInk's licence and those of what it
bundles, shown in the app (Settings, About QuireInk), as binary copies must
carry them. Run it from the repository root after one of them changed; a
test of the `about` plugin checks that each is in the file.

The texts keep their words; their lines are joined into paragraphs, so the
reader sets them like a book.
"""
import re

def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read().strip("\n")

def code_blocks(path):
    return re.findall(r"```text\n(.*?)\n```", read(path), re.S)

def paragraphs(text):
    """The text's paragraphs (separated by empty lines) on one line each;
    lines that start a list item stay apart."""
    out = []
    for block in re.split(r"\n\s*\n", text):
        lines = [l.strip() for l in block.splitlines() if l.strip()]
        joined = []
        for line in lines:
            if joined and not re.match(r"^(- |\* |\([a-z0-9]\) |[0-9]+\. )", line):
                joined[-1] += " " + line
            else:
                joined.append(line)
        # Numbered sections keep their numbers: a no-break space after
        # the number, so Markdown takes them for no list.
        joined = [re.sub(r"^([0-9]+\.) ", "\\1\u00a0", line) for line in joined]
        # Markdown takes a single newline as a space: list items need two.
        out.append("\n\n".join(joined))
    return "\n\n".join(out)

patterns = code_blocks("plugins/hyphenation/patterns/README.md")
parts = [
    ("Notice", read("NOTICE")),
    ("QuireInk: Apache License 2.0", read("LICENSE")),
    ("Literata: SIL Open Font License 1.1", read("plugins/fonts-default/fonts/Literata-OFL.txt")),
    ("Atkinson Hyperlegible Next: SIL Open Font License 1.1",
     read("plugins/fonts-default/fonts/AtkinsonHyperlegibleNext-OFL.txt")),
    ("Tabler Icons: MIT License", read("plugins/fonts-default/fonts/TablerIcons-MIT.txt")),
    ("German hyphenation patterns", patterns[0]),
    ("American English hyphenation patterns", patterns[1]),
]
text = "\n\n".join(f"## {title}\n\n{paragraphs(body)}" for title, body in parts) + "\n"
with open("plugins/about/licences.md", "w", encoding="utf-8", newline="\n") as f:
    f.write(text)
