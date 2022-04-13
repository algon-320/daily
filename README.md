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

You can configure the keybinding by copying `config.yml` to `~/.config/daily/config.yml` and editing it.

By default, the WM uses following keybindings:

|keys|description|
|---------------|-------|
|`Super` + `T`  |Open a terminal window (`/usr/bin/xterm` as default)|
|`Super` + `P`  |Open app launcher (`/usr/bin/dmenu_run` as default)|
|`Super` + `Tab`|Focus the next window|
|`Super` + `J`  |Focus the next monitor|
|`Super` + `K`  |Focus the previous monitor|
|`Super` + `C`  |Close the focused window|
|`Super` + `Space`|Change the layout strategy to the next one|
|`Super` + `1` (num) |Switch to `num`-th (virtual) screen|
|`Super` + `Shift` + `1` (num) |Move the current focused window to `num`-th (virtual) screen|
|`Super` + `Shift` + `Q`  |Quit|
|`Super` + `Up` (`Down`/ `Left` / `Right`)|Move the mouse cursor up / down / left / right|
|`Super` + `Shift` + `Up` (`Down`/ `Left` / `Right`)|Move the mouse cursor **1px** up / down / left / right|
|`Super` + `Enter`|Mouse left-click|

### Layout Specific Keybindings
|layout|keys|description|
|------------------|-------------|-------|
|horizontally tiled|`Super` + `H`|Decrease the width of the leftmost window|
|horizontally tiled|`Super` + `L`|Increase the width of the leftmost window|

## Layout Strategies

- Horizontally tiled
- Vertically tiled
- Full Screen

## Installation

```
$ cargo install --path .
$ vi ~/.xinitrc  # add "exec daily"
```

