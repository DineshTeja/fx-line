# fx-line

Fast inline shell commands and an invisible CMUX voice agent, powered by [fx](https://fx.sh).

## Install

Requires fx, CMUX, and Wispr Flow.

```sh
cargo install --path . --root ~/.local
echo "source \"$PWD/fx-line.zsh\"" >> ~/.zshrc
fx-agent install
```

Set Wispr’s `Fn+Control` shortcut to **Push to talk** while keeping `Fn`. For inline commands, bind `Control+Option` to `text:\x1b[99~` and `Control+Command` to `text:\x1b[100~` in Ghostty.
