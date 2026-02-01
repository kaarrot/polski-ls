# polski-ls - Polish Language Server

A lightweight LSP for Polish text editing in Helix (and other LSP-compatible editors).

# Features

## Spell Checking (Diagnostics)
- Underlines unknown Polish words with HINT severity
- Skips short words (<3 chars) and numbers
- Real-time checking on file open and every change

## Spelling Suggestions (Code Actions)
- Press Space a on an underlined word to see corrections
- Uses fuzzy matching (Levenshtein distance ≤2)
- Quickfix actions replace the word with the selected suggestion

## Autocompletion
- Triggers on any letter (including Polish diacritics: ą, ć, ę, ł, ń, ó, ś, ź, ż)
- Requires 2+ characters typed
- Ranked by: edit distance, prefix match, and word commonness

## Dictionary System
- Embedded baseline: slowa.txt compiled into binary (~150 words)
- User extensions: Any *.txt files in ~/.config/polski-ls/ are loaded at startup
- Word format: One word per line, prefix with * for common words (ranking boost), # for comments

# Design choices
- Support for spellcheck diagnostics with code actions and common words completions while typing (Helix insert mode)
- The default dictionary gets embedded into the binary for easy deployment
- New words can by added as well to a txt file located in ~/.config/polski-ls - one word per line

# Helix setup - languages.toml

```
[language-server.polski-ls]
command = "polski-ls"
args = ["--stdio"]

[[language]]
name = "plaintext"
scope = "text.plain"
file-types = ["txt"]
language-id = "plaintext"
language-servers = ["polski-ls"]
```
