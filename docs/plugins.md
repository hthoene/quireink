# Writing plugins

Everything QuireInk shows and does comes from plugins: the home screen,
the reader, the library, the menus, the settings, even the keyboard. The
kernel only hosts them. A plugin you write can do anything these do, and
replace any of them.

There are two kinds:

- **Data plugins**: a folder with a `plugin.ini` and data files, no code.
  They bring translations (also whole new languages), key maps, new
  defaults for any setting, and fonts. They work on every device as they
  are; see [Plugins without code](#plugins-without-code).
- **Code plugins**: Rust crates against the SDK (`quireink-sdk`). They
  bring screens, actions, settings, formats, widgets, services, anything.
  Today they are compiled into a build of QuireInk; plugins loaded at run
  time (WebAssembly, with the same API) are planned.

This guide walks through a code plugin, then lists what plugins can
contribute. The SDK's API documentation (`cargo doc -p quireink-sdk
--open`) has every detail.

## The sample plugin

[`samples/hello-plugin`](../samples/hello-plugin/src/lib.rs) is a small
plugin to start from. It adds *Hello* to the tools menu, which opens a
screen greeting you by a name set in the settings, with the number of
books opened since the start. Try it in the emulator:

```sh
cargo run -p quireink-emulator --features hello -- --profile boox-go-7 --set hello.name=Ada
```

Open the main menu (a tap at the bottom of the home screen), then
*Tools*, then *Hello*.

### The plugin

A plugin is a type implementing [`Plugin`]: a manifest and a `register`
function. `register` runs once at start-up and adds the plugin's
contributions through a [`Registrar`]:

```rust
pub const PLUGIN: Id = Id::new_static("hello");
pub const OPEN: Id = Id::new_static("hello.open");
pub const SCREEN: Id = Id::new_static("hello.screen");

pub struct Hello;

impl Plugin for Hello {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Hello"),
            description: Cow::Borrowed("A sample plugin to start from."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("hello.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry { menu: menus::TOOLS, item: OPEN, order: 90 })?;
        r.add_icon(OPEN, Cow::Borrowed("star"))?;
        // … the screen, a setting, an event, translations
        Ok(())
    }
}
```

Every id starts with the plugin's id (`hello.open`); `core` is the
kernel's. Titles are translation keys, translated where they are shown.

### Reading and changing: `Env` and commands

Plugins never change the kernel's state directly. They read it through
[`Env`]: the device, the settings, the translations, the fonts, the
storage, the time, the battery, the passwords and keys. They change it
with [`Command`]s: open a screen or slot, go back, change a setting,
emit an event, run an action, show a short message, make an HTTP
request, keep a password, and so on. The kernel runs the commands after
the plugin's code returns.

This keeps plugins simple to test and the API message-based, so the same
model works for sandboxed plugins later.

### Screens

A screen implements [`Screen`]. `draw` paints it; `on_action` reacts to
actions (from buttons, or gestures bound to them); `on_gesture` to taps,
long presses and swipes; `on_event` to events while it is open:

```rust
impl Screen for HelloScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let title = env.i18n.tr("hello.title");
        self.header.draw(canvas, env, &self.style, title, None, None);
        // … the greeting, centred below
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        if *action == actions::CONFIRM || *action == actions::BACK {
            cx.command(Command::Back);
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        // A tap on the header's back arrow, or anywhere else, closes it.
    }
}
```

Screens are made by a factory each time they open
(`r.add_screen(ScreenSpec { id, title, factory })`). Return
`Handled::No` for what a screen does not handle: the kernel then runs
the action's own handler (e.g. the menu button opens the menu).

Some rules every screen should follow:

- **Touch and buttons alike.** Some devices have only buttons (the
  Xteink X4), some mostly touch (the Boox). Everything must be easy with
  either: buttons move a focus and confirm, taps act at once. Show the
  focus only once buttons are used (`ListView::focus_on_keys`).
- **`draw` must not change state.** On small devices a frame is drawn in
  bands, so `draw` may run several times for one frame. It can skip what
  lies outside `canvas.clip()`.
- **Ask for redraws.** After a change, `cx.request_redraw()` (or a part
  of the screen) and `cx.request_refresh(mode)`: `Fast` for a moving
  focus, `Partial` for new content, `Full` to clear ghosting.
- **Use the SDK's UI parts** (`quireink_sdk::ui`): headers, rows, lists,
  switches, numbers with − and +, messages, icons. They follow the user's
  appearance settings and line up with the rest of QuireInk.

Screens say what they show, for reading the interface aloud,
automation and tests: the `ui` helpers (list rows, buttons, switches,
headers, labels) tell what they draw by themselves. A screen that draws
in its own way tells it with `canvas.describe(role, text, area,
focused, on)`.

### Settings

```rust
r.add_setting(
    SettingSpec::new(NAME, "hello.setting.name", SettingKind::Text,
        SettingValue::Text(Cow::Borrowed("")))
        .group(groups::OTHER)
        .description("hello.setting.name.help"),
)?;
```

The settings screens list every setting by its group, with its
description below it; the kernel saves changes to `quireink.ini`. Read
a setting with `env.settings.text(…)`, `.bool(…)` or `.int(…)`. Settings
can be numbers with units and steps, switches, choices, languages or
text; `advanced()` hides rarely needed ones, `scoped()` lets one be set
per book, `editor(screen)` opens a screen of your own instead. Passwords
and keys do not belong in settings: see [Passwords and keys](#passwords-and-keys).

### Events

```rust
r.subscribe(events::BOOK_OPENED, Box::new(move |env, params, out| {
    opened.set(opened.get() + 1);
}))?;
```

The kernel and plugins emit events with parameters; any plugin can
listen. Standard ones are in `builtin::events`: started, a setting
changed, a book opened, a page turned, a book moved, its bookmarks or
highlights changed, a word looked up, the device suspends or resumes.
Plugins define their own (the reader's `reader.notes-exported`, for
example), and `Command::Schedule` emits one at a time of day. Open
screens get every event too (`Screen::on_event`).

What is known of books is a service, not a screen's: `library` has a
book's details (title, author, series, tags, cover, progress, and
fields plugins add with `library::set_field`), the books read last and
the favourites, so any plugin can show or use them.

The bookmarks and highlights of books are shared: `annotations` reads
and keeps them, and whoever changes them emits
`annotations::changed(book)`, so the reader shows them anew and syncs
pass them on (the `koreader` plugin, for example).

State a plugin keeps between events lives in an `Rc` it hands to its
handlers when it registers.

### Translations

```ini
[meta]
language = en
name = English
plural = one-other

[messages]
hello.title = Hello
# {name} is the name set, {count} the books opened since the start.
hello.greeting.one = Hello, {name}! You opened {count} book.
hello.greeting.other = Hello, {name}! You opened {count} books.
```

Every text a plugin shows comes from catalogs like this, added with
`r.add_translations(…)`; English first, then any other language. `tr`
translates, `format` fills `{placeholders}`, `plural` picks the form for
a number. A test that all languages have the same keys belongs in every
plugin (see the sample).

### Testing

Plugins are tested with the real kernel and an in-memory storage, as the
sample's [test](../samples/hello-plugin/tests/hello.rs) does: build a
kernel with the plugin, send it input, look at the open screen, the
settings, the files, the HTTP requests it makes (`Kernel::take_requests`)
and answer them (`Kernel::http_response`). The emulator then shows how it
looks and how much memory it needs on each device:

```sh
cargo run -p quireink-emulator -- --profile xteink-x4 --headless --script "confirm" --screenshot out.png
```

## What plugins contribute

| Contribution | Registrar | What it is |
|---|---|---|
| Actions | `add_action` | Something to do, bound to buttons and gestures, listed in menus |
| Screens | `add_screen` | Full screens and overlays (menus, dialogs, panels) |
| Slots | `add_slot`, `bind_slot` | Roles a screen fills: home, reader, library, menu, settings, keyboard, dictionary, … |
| Menu entries and sections | `add_menu_entry`, `add_menu_section` | Actions, settings and submenus in any menu |
| Icons | `add_icon` | The icon of an action, setting group or menu |
| Settings and groups | `add_setting`, `add_setting_group` | Shown and saved by the kernel |
| Setting defaults | `set_setting_default` | New defaults for other plugins' settings (themes) |
| Event handlers | `subscribe` | React to events |
| Key maps | `add_keymap`, `add_keymap_preset` | What buttons, taps and swipes do |
| Formats | `add_format` | Open a kind of file as a document to read |
| Fonts | `add_font`, `add_font_file`, `bind_font` | Fonts and the roles they fill |
| Hyphenation | `add_hyphenation` | Patterns for a language |
| Text filters | `add_text_filter` | Change a book's text before it is laid out |
| Input filters | `add_input_filter` | See every button and finger first: pass it, take it, or run an action instead |
| Refresh filters | `add_refresh_filter` | Change how the panel is refreshed, e.g. in full on a screen of your own |
| Status items and lines | `add_status_item`, `add_status_line` | What status lines show |
| Widgets | `add_widget` | Blocks for the home screen and the sleep screen |
| Translations | `add_translations` | Catalogs for any language |

Slots are how a plugin replaces another: bind your screen to
`core.home` and it is the home screen; the user's
`[slots]` configuration has the final word. Built-in slots are in
`builtin::slots`, built-in menus in `builtin::menus`: the main menu
(`core.main`), its tools (`core.tools`) and the settings' pages
(`core.settings-pages`).

### Widgets with settings

A widget may have settings of its own, such as where its text sits. Put
them in a settings group with the widget's id
(`widgets::settings_group`); the settings list leaves such groups out,
and the home screen's editor offers them as the widget's *Settings*. For
the alignment there is a ready setting: register
`widgets::align_setting(&id, Align::Left)` and place each text with
`widgets::align(env, id).x(area, width)`.

Screens that place widgets one below the other should know the kernel's
two widgets for room between them: `widgets::SPACE` is an ordinary
widget, a gap; `widgets::FILL` has no height of its own and takes its
share of the room the others leave (`widgets::fill_height`). Both may
be in a list more than once (`widgets::repeatable`).

### Passwords and keys

Plugins keep passwords, API keys and tokens with
`Command::SetSecret(SecretChange::keep(id, label, value))` and read them
with `env.secrets.get(id)`. They are all listed in *Settings, Passwords
and keys*, where the user changes or removes them. Ask for one on the
keyboard in password mode (`params::MODE` `password`); the keyboard
offers to type it on a phone. They are kept apart from the settings and
not readable at a glance, but not encrypted: the devices have no safe
place for a key.

### The network

`Command::Http` makes a request where the device has a network
(`env.network`); the answer comes as the request's reply event, with
the status and the body, or saved to a file (`save_to`) for large ones.
Read large JSON answers as they stream with `json::stream`, so they
need little memory. `Command::Serve` serves HTTP to the local network,
e.g. for transfers from a computer or typing on a phone.

## Memory

The smallest device, the Xteink X4, has 192 KiB of heap for everything.
A plugin that should run there:

- keeps little in memory: lists in files, read as they are needed;
- reads files a piece at a time (`Storage::read_at`), never whole books;
- writes downloads straight to storage (`HttpRequest::save_to`);
- reserves large buffers with `try_reserve` and copes when that fails;
- checks `env.device.memory` (`MemoryClass::Small`) to do less there.

The emulator's X4 profile enforces the budget, and `--heap-report` shows
where memory goes.

## Plugins without code

A data plugin is a folder:

```text
plugins/large-print/
  plugin.ini     [plugin] id, name, description, version
  i18n/*.ini     translation catalogs (also whole new languages)
  keymap.ini     key bindings
  settings.ini   new defaults for any plugin's settings ([reader] font-size = 14)
  fonts/*.ttf    fonts to read with (also .otf, .ttc), by family
  styles.css     a stylesheet for books, after a book's own (EPUB)
```

Copy it into the `plugins` folder of a device (or of the emulator's
library) and restart. A plugin's `styles.css` comes after a book's own
stylesheets and before one's own (`styles.css` beside the books, then
the one named like a book), so the more particular wins; plugins with
code add theirs with `Registrar::add_stylesheet`. [`samples/plugins`](../samples/plugins) has two:
large print and an airy theme.

[`Plugin`]: ../crates/quireink-sdk/src/plugin.rs
[`Registrar`]: ../crates/quireink-sdk/src/plugin.rs
[`Env`]: ../crates/quireink-sdk/src/env.rs
[`Command`]: ../crates/quireink-sdk/src/command.rs
[`Screen`]: ../crates/quireink-sdk/src/screen.rs

Other protocols go over sockets ([ADR 0014](adr/0014-sockets-for-plugins.md)):
`Command::Connect` opens a TCP connection, `Command::SocketSend` sends,
and `Command::SocketRead` asks for what comes next, as text
(`SocketRead::Text`) or as a file of a known length written to storage as
it comes (`SocketRead::File`), so files of any size never need to fit in
memory. `Command::Broadcast` finds services in the local network by a
UDP datagram and their answers. Everything comes back as events
(`socket_params`); a socket closes when the screen showing as it was
opened is closed.

