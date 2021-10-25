# Daily

Daily is a X11 window manager for (my) daily use.

## Usage

Try Daily with a dedicated X server:
```
$ # open a virtual console (e.g. by hitting Ctrl + Alt + F5)
$ cargo build --release
$ startx $PWD/target/release/daily
```

Or, thanks to Xephyr you can try Daily under another Window Manager: 
```
$ Xephyr -screen 960x540 :2  # open a virtual display
$ DISPLAY=:2 cargo run --release
```

## Keybindings

|keys|command|
|---------------|-------|
|`Super`        |Show window border while pressing the key|
|`Super` + `T`  |Open a terminal window|
|`Super` + `P`  |Open app launcher (`/usr/bin/dmenu_run` as default)|
|`Super` + `Tab`|Focus the next window|
|`Super` + `J`  |Focus the next monitor|
|`Super` + `C`  |Close the focused window|
|`Super` + `1` (num) |Switch (virtual) screen|
|`Super` + `Shift` + `1` (num) |Move the focused window to the specified (virtual) screen|
|`Super` + `Shift` + `Q`  |Quit|

## Layout Strategies

- Horizontally tiled

## Installation

```
$ cargo install --path .
$ vi ~/.xinitrc  # add "exec daily"
```

