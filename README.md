# CCDisplay

Basically just a series of tubes funneling video in -> video out and audio in
-> audio out. Works on, well, Linux with pulseaudio at least. Video might work
elsewhere, audio definitely will not. Has some niceties such as a config window
and auto-reconnect for if the capture card gets nudged. (If you can't tell,
this is a tool I made for myself. I'm happy to accept PRs for changes that help
make it work for you, though!)

![A screenshot of CCDisplay's settings window. There are 3 options visible:
Window title, a text field set to "Splatoon 3"; and Video source and Audio
source, both dropdowns displaying names of peripherals](settingswindow.png)

## Keyboard shortcuts

| Key   | Function                                           |
| ----- | -------------------------------------------------- |
| Esc   | Quit                                               |
| F     | Fullscreen                                         |
| Alt-S | Open settings (might not work at first; winit bug) |

## License

This project is licensed under the MIT license. Please see the
[LICENSE](LICENSE) file for more details.
