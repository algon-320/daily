# Daily

Daily is a X11 window manager for (my) daily use.

## Usage

Try Daily with a dedicated X server:
```
$ # open a virtual console (e.g. by hitting Ctrl + Alt + F5)
$ cargo build --release
$ startx target/release/daily
```

Or, thanks to Xephyr you can try Daily under another Window Manager: 
```
$ Xephyr -screen 960x540 :2  # open a virtual display
$ DISPLAY=:2 cargo run --release
```

## Install

```
$ cargo install --path .
$ vi ~/.xinitrc  # add "exec daily"
```

