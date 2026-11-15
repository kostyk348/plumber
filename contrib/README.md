# contrib — как поставить plumber в систему

Эти файлы не часть библиотеки: это то, без чего «установил и работает» не получается.
Демон — половина системы, вторая половина — читатели портов, автозапуск и обработчик файлов.

## Быстрая установка (Linux, XDG)

```sh
# 1. бинарники
cargo build --release
install -m 755 target/release/{plumberd,plumb,plumber-open} ~/.local/bin/

# 2. правила
install -Dm644 plumbing.example ~/.config/plumber/plumbing

# 3. сценарии из contrib
install -m 755 contrib/plumber-session contrib/plumb-file ~/.local/bin/
install -Dm644 contrib/autostart/plumber-session.desktop ~/.config/autostart/
install -Dm644 contrib/applications/plumber-file.desktop ~/.local/share/applications/
update-desktop-database ~/.local/share/applications

# 4. обёртки для fish (для bash/zsh — те же три функции руками)
install -Dm644 contrib/fish/plumber.fish ~/.config/fish/conf.d/plumber.fish

# 5. запустить сейчас и проверить
plumber-session start
plumber-session status
```

Дальше — `plumb send "src/main.rs:42"` из любого терминала и `pl <текст>` из fish.

## systemd вместо автозапуска

Если на машине есть `systemd --user`, положите `contrib/systemd/plumber.service` в
`~/.config/systemd/user/` и включите:

```sh
systemctl --user daemon-reload
systemctl --user enable --now plumber
```

Читатели портов в этом случае запускаются отдельно (или тем же `plumber-session start`).

## Про обработчик файлов

`plumber-file.desktop` по умолчанию **не** назначается обработчиком `text/plain` — и это
намеренно: `plumb-file` при выключенном демоне откатывается на `xdg-open`, а если он же и есть
обработчик по умолчанию, получится петля. Защита от неё в `plumb-file` есть (`PLUMB_FILE_ACTIVE`),
но правило «не ставить себя обработчиком самого себя» надёжнее защиты в коде.

Безопасно и полезно назначить его обработчиком схемы:

```sh
xdg-mime default plumber-file.desktop x-scheme-handler/plumb
```

Тогда `plumb://src/main.rs:42` в любом приложении открывается через правила.

## Переменные

- `PLUMBER_BIN` — каталог с бинарниками (по умолчанию `~/.local/bin`).
- `PLUMBER_RULES` — файл правил (по умолчанию `~/.config/plumber/plumbing`).
- `PLUMBER_PORTS` — порты, на которых держать читателей (по умолчанию `edit web pdf`).
- `PLUMBER_DAEMON_FLAGS` — дополнительные флаги демона (например `--allow-start`).
