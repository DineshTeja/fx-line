# fx-line

Inline natural-language commands for zsh in CMUX, powered by [Vercel fx](https://fx.sh). Control+Option toggles the input, Control+Command inserts a command, and Ctrl+C cancels.

## Install

Requires an installed and authenticated `fx`.

```sh
cargo install --path .
echo "source \"$PWD/fx-line.zsh\"" >> ~/.zshrc
```

Add this CMUX/Ghostty binding:

```ini
keybind = ctrl+alt+left_alt=text:\x1b[99~
keybind = ctrl+alt+left_control=text:\x1b[99~
keybind = ctrl+super+left_super=text:\x1b[100~
keybind = ctrl+super+left_control=text:\x1b[100~
```
