# plumber.sh — обёртки для bash и zsh. Положить в ~/.bashrc или ~/.zshrc:
#   . ~/.config/plumber/plumber.sh

if command -v plumb >/dev/null 2>&1; then
    # pl <текст>            — отправить текст по правилам
    pl() { plumb send "$*"; }
    # plf <файл> [строка]   — открыть файл на строке
    plf() { plumb send "${1}:${2:-1}"; }
    # plc                   — состояние демона и читателей
    plc() { plumber-session status; }
    # pll [n]               — последние n сообщений журнала
    pll() { plumb log -n "${1:-15}"; }
fi
