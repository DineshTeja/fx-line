# fx-line

Inline natural-language commands for zsh in CMUX, powered by [Vercel fx](https://fx.sh). Press ⌘K, type a request, and press Enter to insert the command for review.

## Install

Requires an installed and authenticated `fx`.

```sh
cargo install --path .
echo "source \"$PWD/fx-line.zsh\"" >> ~/.zshrc
```

Add this CMUX/Ghostty binding:

```ini
keybind = super+k=text:\x1b[99~
```
