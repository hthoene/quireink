# Changelog

What changes from one release of QuireInk to the next. Versions follow
[Semantic Versioning](https://semver.org); before 1.0, a minor version may
change the plugin SDK.

## [Unreleased]

- A new icon: the capital Q of Besley in ink on e-paper, as on
  quireink.org (its source is `shells/android/icon.svg`).
- X4: SD cards with a partition table (MBR or GPT), as nearly all come,
  are read; before, only cards with the file system from the first block
  were. A card in exFAT is named as such in the log.
- X4: remote control over USB for testing on the device:
  `tools/x4-remote.py` presses its buttons, takes screenshots and tells
  what the screen shows, with the steps of the emulator's scripts.
  `Kernel::capture` draws the whole screen for a shell to send elsewhere.
- What the buttons do, at the bottom of the screen: where buttons lie in
  a row below it (the X4's four), a row above them names what each does
  on the screen shown: *Back*, *Select*, arrows, or a screen's own word.
  Not in the reader, which keeps the whole page; switched off under
  Controls › *Show what the buttons do*. Screens name their buttons with
  `Screen::hint`, shells give the row with `KernelBuilder::button_row`.
- X4: on USB power, sleep shows the sleep screen and keeps the USB
  connection, so a computer can still read the log and wake it.

## [0.3.0] - 2026-10-16

- Screen lock (off by default): a PIN, a pattern or a password asked for
  when QuireInk starts and when the device wakes up, set under System ›
  *Screen lock*. The code is kept only as a salted hash. Locked, nothing
  else can be used; after five wrong tries the next one waits half a
  minute. *Lock now* locks at once from any button, gesture or rule.
  It looks like a phone's: the time and the date above, round keys
  below, rings for the digits still to come.
- `Canvas::draw_line`.
- Plugin SDK: input filters (`Registrar::add_input_filter`) see every
  button and finger before screens and key maps do, and may take it or
  run an action instead; refresh filters (`Registrar::add_refresh_filter`)
  change how the panel is refreshed. `InputEvent` and `ButtonState` are
  part of the SDK.

- Plugin SDK: services, what one plugin offers others
  (`Registrar::add_service`, `Env::services`), and points plugins define
  for others to contribute to (`Registrar::contribute`); the event
  `core.library-changed` (`library::changed`), emitted when books come
  into the library or leave it (Wi-Fi transfer, catalogs, Calibre, the
  file manager), and the library shows them at once.
- Rules can have a condition: only between two times of day, only while
  reading or while not, only on battery or while charging, only while a
  switch is on or off (hold a rule, *Only if …*). They can also set a
  number (e.g. the light to 20) and show a message.

- Drag and drop in lists, as on phones: entries of menus (*Customise
  menus*), the widgets of the sleep screen and the items of status lines
  carry a grip (≡) at their end; drag it to move the row, a bar shows
  where it lands. The status line's chosen items now come first, in
  their order. Buttons move rows as before.
- Plugin SDK: `ui::Reorder` (rows moved by dragging a grip, for any
  `ListView`), and screens get a finger before the swipes in from the
  edges do (`Screen::on_pointer`), so grips and handles at the edges
  work.
- The quick panel has its name in *Customise menus*.

- Rules know many more moments: at a time of day on chosen days (every
  day, Monday to Friday, at weekends, one day), every so many minutes,
  when a book is closed or read to its end, when the device goes to
  sleep, when the battery falls below a percentage, when charging begins
  or ends, and when a switch is turned on or off.
- Plugin SDK: events `core.book-closed` and `core.book-finished` (the
  reader emits them), and `Screen::on_close`, called when a screen is
  closed.

- Gestures as on phones: up from the bottom edge goes home, down from the
  top edge opens the quick panel, in from a side goes back (while reading
  it turns the page). The quick panel holds night mode, the light,
  turning the screen, refreshing the screen, a screenshot, sleep and the
  settings, with the time, date and battery above (what shows there and
  how large is set under Appearance); plugins and the menu editor change
  what it holds. *Refresh the screen* (`core.refresh-screen`) clears
  e-ink ghosting with a full refresh, from the quick panel or any button
  or gesture it is bound to. Swipes in from the edges come before the screen sees them.
- More gestures as on phones: up on the home screen shows every tool at
  once (*All tools*, `home.all-tools`), down on it searches the library,
  and two fingers apart or together make the text larger or smaller
  while reading (`pinch.out`, `pinch.in`, in a section *Two fingers* of
  *Buttons and gestures*). The first home screen on a touch screen
  explains the gestures once (*Hint on the gestures* under Appearance
  shows it again).
- Gestures per tool: as standard, the tool's own (gestures go to it, a
  swipe in from an edge needs a second one: for games and drawing) or
  off. A tool sets its default (`gestures.policy` in its key map); the
  user changes it in *Buttons and gestures*.
- *Buttons and gestures* is easier to find one's way in: where a change
  applies (everywhere, home screen, reading, library, any tool), inputs
  in sections (edges, swipes, taps with a picture of the zone, held
  taps, buttons, held buttons), and the actions to choose from in groups
  with a search, *As standard* and *Nothing* first.

- The home screen is a grid, as on a phone: 4 columns by default (rows
  to fit the screen; both settings), each widget taking a rectangle of
  cells, each tool an icon in a cell of its own; empty cells stay empty.
  A long press or the pencil shows the grid: drag an item to move it, by
  its corner to size it, tap it for its options (move or size it with
  the buttons, its settings, remove it); *Add* adds widgets, tools and
  pages. The status line sits above the grid. Pages written before are
  placed in the grid by themselves.
- Widgets have more settings of their own: the clock's size (also as
  large as its cells allow), the date's size, whether the continue card
  shows its cover, its progress and a frame, and for the recently read
  books their heading, their titles, where the titles sit and how many
  books show. Tools with or without their names.

## [0.2.0] - 2026-10-16

- Books from a catalog (OPDS) or from Calibre keep the title, author,
  series and tags the source gives, where the book names none itself
  (text files, PDFs): `library::provide`.
- *Say what is chosen* (System, where the device speaks): says the row
  or button the buttons move to, with a switch's state, for using
  QuireInk without looking at the screen.
- Screens say what they show (PLANNING G11): rows with their text and
  focus, buttons, switches and headings, told by the drawing helpers as
  they draw (`Canvas::describe`, `semantics::parts`,
  `Kernel::describe`). The emulator's scripts tap by name
  (`tap-on:Display`) and print a screen's parts (`describe`).
- The library is a service (`library` in the plugin SDK): what is known
  of books (title, author, series, tags, cover, progress, and fields
  plugins add), the books read last and the favourites, kept by the
  kernel from the reading events. The library, the home screen and the
  sleep screen show the same data instead of keeping copies of their
  own; what the library kept before is taken over.
- Bookmarks and highlights shared with KOReader (`koreader` plugin), for
  books read in both on one device or in one synced library folder:
  what KOReader has in `Book.sdr` comes over when a book is opened, and
  what is new here goes to KOReader the next time it opens the book;
  removing one, or a later note, goes across too. EPUB only.
- EPUB positions as KOReader writes them, down to the character: reading
  positions synced with KOReader land on the word, not the paragraph.
- Plugin SDK: bookmarks and highlights are kept by `annotations`, for
  every plugin, with when they were made and changed; event
  `core.annotations-changed`. `Document::xpointer` and
  `Document::locate_xpointer` are exact to the character.
- Colour pictures on colour screens (ADR 0015): covers, pictures in
  books, comics, OPDS covers and the sleep screen's pictures are shown
  in colour on phones and tablets and on colour e-ink (Boox models named
  "Color" or "C", with colours strengthened for the filter); text and
  the interface stay gray, monochrome e-ink is unchanged. JPEG (also
  progressive), PNG and GIF are decoded in colour where asked. Plugin
  SDK: `ImageSink::wants_color` and `row_rgb`, `ColorFile`,
  `ColorWriter`, `ImageFile`, `image::picture_from_file`,
  `OpenOptions::color`, `Formats::options`, `Canvas::draw_rgb_row`,
  `Framebuffer::set_saturation`, `ColorPanel::boost_percent`.
- Smaller text by default: menus 9 pt (was 11), status lines 6 pt
  (was 8). Both stay adjustable in the settings.
- Dictionary: the page of a long entry no longer covers *Online*.
- Text and paper colours on Android (Appearance): eight of each, or
  one's own colour code (e.g. #5b4636); night mode swaps them.
- Room between widgets: *Space*, a gap (small, medium or large), and
  *Free space*, which takes the room the others leave, e.g. to put the
  clock in the middle or the tools at the bottom. Both may be added more
  than once, on the home screen and the sleep screen.
- Appearance: *Corners* (square, slightly rounded, round) for
  highlighted rows, buttons and boxes, and *Contrast*: strong draws gray
  hints and notes in black, for screens with few grays. The Airy theme
  sample rounds its corners. Plugin SDK: `UiStyle` gains `corner` and
  `strong` (with `UiStyle::faint` for gray text), `ui::fill_round`,
  `ui::stroke_round`, and the canvas rounded rectangles.
- GIF pictures in books, comics and the sleep screen's own pictures: the
  first picture of the file (also of an animation, interlaced or with
  transparency), decoded to gray row by row like JPEG and PNG.
- Plugins can bring a stylesheet for books: a data plugin's
  `styles.css`, or `Registrar::add_stylesheet`, applied after a book's
  own and before one's own (EPUB). SDK: `Env::stylesheets`,
  `OpenOptions::css` (no longer `Copy`).
- Books' embedded fonts in WOFF are used too (WOFF 2 is not read).
- Calibre's wireless device connection (`calibre` plugin, in the tools):
  the device finds Calibre in the network (or connects to the address
  set), and Calibre sends books to the library, lists and deletes them
  as with a reader on a cable; a password set in Calibre is asked for
  once. SDK: `sha1`, and `json::raw_member`, `json::unquote`,
  `json::string_array`.
- Sockets for plugins (ADR 0014): TCP connections that read text or
  files of any size into storage, and UDP broadcasts to find services,
  in the emulator and on Android (`Command::Connect`, `SocketSend`,
  `SocketRead`, `Disconnect`, `Broadcast`).
- Small capitals (CSS `font-variant: small-caps`, `font-variant-caps`)
  in EPUB, HTML and the other converted formats: lower case letters as
  smaller capitals, the text itself unchanged (search, selection and
  notes as before). Books are prepared anew once. Document model:
  `TextStyle::SMALL_CAPS`; embedded font families at most 31 (was 63).
- Snapshot tests: screens of the X4 and the Boox are drawn headless and
  compared with reference pictures, so layout faults fail a test.
- The folder chooser says when it is still opening a folder (slow SD
  cards) and ignores taps until it shows it.
- Widgets have settings of their own, from the home screen's editor:
  where the status line, date, clock, today's reading and the word of
  the day sit (left, centre, right), and whether the clock shows the
  date. Plugins give their widgets settings the same way
  (`widgets::settings_group`, `widgets::align_setting`).

## [0.1.1] - 2026-10-15

The Android app for F-Droid.

- The app is built reproducibly: with the same tools (shells/android/
  versions.env) it comes out byte for byte the same anywhere, so F-Droid
  can publish it signed with QuireInk's key. A dependency's macro that
  wrote its code in a random order is patched (patches/jni-macros).
- The app's library is stored in the APK and runs from there, without
  symbols: 6.8 MB to download, less space taken on the device.
- The licence names the copyright holder; README with screenshots.

## [0.1.0] - 2026-10-15

The first release: QuireInk on Android e-readers (tested on a Boox Go 7)
and in the desktop emulator, with its plugin SDK. The firmware for the
Xteink X4 is built but has not run on a device yet.

### Reading

- EPUB 2 and 3 (with the book's styles, embedded fonts and pictures, or
  one's own stylesheets per folder or book), FB2, Markdown, plain text,
  comic archives (CBZ), Xteink's XTC and XTCH, and PDF on Android.
- Contents, search, bookmarks and highlights with notes, export of notes
  as Markdown, a dictionary (StarDict), Wikipedia and translation
  (DeepL), a vocabulary list of words looked up.
- Hyphenation (German, English), reading fonts with weight and size,
  margins, spacing, status lines to taste, reading aloud, pages turned
  by themselves, turning the screen.
- Reading statistics, shared with KOReader (also over WebDAV), and
  reading positions synced with KOReader's sync server.

### Books and more

- A home screen of widgets on pages (date, battery, continue reading,
  recently read, tools, statistics, headlines), editable by touch or
  buttons.
- The library by folders, authors, series and tags; a file manager;
  books from OPDS catalogs and WebDAV folders (Nextcloud), kept in sync.
- News feeds as books; Wallabag's read-later articles, highlights to
  Readwise and a browser, switched off until wanted; transfers from a
  computer or phone over Wi-Fi, typing on a phone.
- A sleep screen (cover, picture or widgets), screenshots.

### Making it one's own

- Every screen, menu, key and gesture can be changed, in the app or in
  `quireink.ini`; profiles, rules, themes, translations and fonts as
  plugins without code (data plugins).
- Settings with descriptions and a search, passwords and keys in one
  place, short menus by default, usable by touch and by buttons alike.
- English and German.

### For developers

- The plugin SDK (`quireink-sdk`) with a guide ([Writing
  plugins](docs/plugins.md)) and a sample plugin; plugins are tested
  against the real kernel.
- The desktop emulator with device profiles and memory budgets.
- The Xteink X4's firmware, its display, buttons, battery and SD card
  tested against simulated hardware ([ADR 0013](docs/adr/0013-x4-firmware.md)).

[0.3.0]: https://github.com/hthoene/quireink/releases/tag/v0.3.0
[0.2.0]: https://github.com/hthoene/quireink/releases/tag/v0.2.0
[0.1.1]: https://github.com/hthoene/quireink/releases/tag/v0.1.1
[0.1.0]: https://github.com/hthoene/quireink/releases/tag/v0.1.0
