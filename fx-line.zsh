#!/usr/bin/env zsh

[[ -o interactive ]] || return 0 2>/dev/null || exit 0

autoload -Uz add-zsh-hook

typeset -gi _FX_LINE_ACTIVE=0
typeset -g _FX_LINE_SAVED_BUFFER=''
typeset -gi _FX_LINE_SAVED_CURSOR=0
typeset -g _FX_LINE_SAVED_PROMPT=''
typeset -g _FX_LINE_SAVED_RPROMPT=''
typeset -g _FX_LINE_SAVED_KEYMAP='main'

_fx_line_binary() {
  if [[ -n ${FX_LINE_BIN:-} && -x $FX_LINE_BIN ]]; then
    print -r -- "$FX_LINE_BIN"
  elif (( $+commands[fx-line] )); then
    print -r -- "$commands[fx-line]"
  elif [[ -x $HOME/.local/bin/fx-line ]]; then
    print -r -- "$HOME/.local/bin/fx-line"
  else
    return 1
  fi
}

_fx_line_restore() {
  PROMPT=$_FX_LINE_SAVED_PROMPT
  RPROMPT=$_FX_LINE_SAVED_RPROMPT
  POSTDISPLAY=''
  _FX_LINE_ACTIVE=0
  zle -K "$_FX_LINE_SAVED_KEYMAP" 2>/dev/null || zle -K main
}

_fx_line_cancel() {
  (( _FX_LINE_ACTIVE )) || return 0

  local buffer=$_FX_LINE_SAVED_BUFFER
  local cursor=$_FX_LINE_SAVED_CURSOR
  _fx_line_restore
  BUFFER=$buffer
  CURSOR=$cursor
  zle reset-prompt
}

_fx_line_open() {
  if (( _FX_LINE_ACTIVE )); then
    _fx_line_cancel
    return
  fi

  _FX_LINE_SAVED_BUFFER=$BUFFER
  _FX_LINE_SAVED_CURSOR=$CURSOR
  _FX_LINE_SAVED_PROMPT=$PROMPT
  _FX_LINE_SAVED_RPROMPT=${RPROMPT-}
  _FX_LINE_SAVED_KEYMAP=${KEYMAP:-main}
  _FX_LINE_ACTIVE=1

  PROMPT='fx › '
  RPROMPT=''
  BUFFER=''
  CURSOR=0
  POSTDISPLAY=''
  zle -K fx-line
  zle reset-prompt
}

_fx_line_submit() {
  local request=$BUFFER
  local saved_buffer=$_FX_LINE_SAVED_BUFFER
  local saved_cursor=$_FX_LINE_SAVED_CURSOR
  local binary command_text error_text exit_status

  if [[ -z ${request//[[:space:]]/} ]]; then
    POSTDISPLAY='  type a request'
    zle -R
    return
  fi

  binary=$(_fx_line_binary) || {
    _fx_line_restore
    BUFFER=$saved_buffer
    CURSOR=$saved_cursor
    zle reset-prompt
    zle -M 'fx-line is not installed'
    return 1
  }

  POSTDISPLAY='  …'
  zle -R
  command_text=$(command "$binary" "$request" "$PWD" "$saved_buffer" 2>&1)
  exit_status=$?

  _fx_line_restore
  if (( exit_status == 0 )) && [[ -n $command_text ]]; then
    BUFFER=$command_text
    CURSOR=${#BUFFER}
  else
    error_text=${command_text#fx-line: }
    BUFFER=$saved_buffer
    CURSOR=$saved_cursor
  fi
  zle reset-prompt

  (( exit_status == 0 )) || zle -M "fx-line: ${error_text:-generation failed}"
}

_fx_line_precmd() {
  if (( _FX_LINE_ACTIVE )); then
    PROMPT=$_FX_LINE_SAVED_PROMPT
    RPROMPT=$_FX_LINE_SAVED_RPROMPT
    POSTDISPLAY=''
    _FX_LINE_ACTIVE=0
  fi
}

zle -N fx-line-open _fx_line_open
zle -N fx-line-submit _fx_line_submit
zle -N fx-line-cancel _fx_line_cancel

bindkey -N fx-line emacs
bindkey -M fx-line '^M' fx-line-submit
bindkey -M fx-line '^J' fx-line-submit
bindkey -M fx-line '^[' fx-line-cancel
bindkey -M fx-line '^C' fx-line-cancel
bindkey -M fx-line '^G' fx-line-cancel
bindkey -M fx-line $'\e[99~' fx-line-cancel
bindkey -M fx-line $'\e[100~' fx-line-submit

bindkey -M emacs $'\e[99~' fx-line-open 2>/dev/null
bindkey -M viins $'\e[99~' fx-line-open 2>/dev/null
bindkey -M vicmd $'\e[99~' fx-line-open 2>/dev/null

add-zsh-hook -d precmd _fx_line_precmd 2>/dev/null
add-zsh-hook precmd _fx_line_precmd
