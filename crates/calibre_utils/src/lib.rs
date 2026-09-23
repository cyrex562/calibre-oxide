pub mod certgen;
pub mod cleantext;
pub mod config;
mod config_tests;
pub mod constants;
pub mod copy_files;
pub mod date;
pub mod exim;
pub mod ffml_processor;
pub mod file_type_icons;
pub mod filenames;
pub mod fonts;
pub mod formatter;
pub mod html2text;
pub mod hyphenation;
pub mod icu;
pub mod imageops;
pub mod imghdr;
pub mod localization;
pub mod localunzip;
// Linux-only: `flock`/`geteuid` plus an abstract-namespace Unix domain
// socket (`std::os::linux::net::SocketAddrExt`), which no other platform
// has. See the module's own doc -- the Windows and macOS/BSD branches of
// upstream's `lock.py` were deliberately not ported.
#[cfg(target_os = "linux")]
pub mod lock;
pub mod logging;
pub mod lzx;
pub mod matcher;
pub mod mem;
pub mod monotonic;
pub mod mreplace;
pub mod msdes;
pub mod network;
// Unix-only: its one real submodule is a freedesktop.org `.desktop`
// file reader (`linux.py`). `osx.py`/`windows.py` were never ported --
// see the module doc -- so there is nothing here for Windows to reach.
#[cfg(unix)]
pub mod open_with;
pub mod opensearch;
pub mod ordered_dict;
pub mod podofo;
pub mod podofo_dedup_images;
pub mod podofo_fonts;
pub mod podofo_impose;
pub mod podofo_merge;
pub mod podofo_outline;
pub mod podofo_pages;
pub mod pool;
pub mod quantize;
pub mod net_guard;
pub mod random_ua;
pub mod recycle_bin;
pub mod resources;
pub mod safe_atexit;
pub mod search_query_parser;
pub mod series;
pub mod seven_zip;
pub mod short_uuid;
pub mod smartypants;
pub mod smtp;
// Unix-only: the Windows half was only ever a stub that silently did
// nothing (`set_socket_inherit` was a no-op and `get_socket_inherit`
// always answered `false`). Nothing in this workspace calls either,
// so an honestly-absent module beats a shipped no-op that would look
// like it worked. Wants a real `SetHandleInformation` implementation
// before it comes back.
#[cfg(unix)]
pub mod socket_inheritance;
pub mod speedups;
// Unix-only: POSIX `fcntl` record locks (`libc::flock`, `F_SETLK`), which
// have no Windows equivalent here.
#[cfg(unix)]
pub mod tdir_in_cache;
pub mod terminal;
pub mod text2int;
pub mod titlecase;
pub mod translations;
pub mod unicode_names;
pub mod unrar;
pub mod unsmarten;
pub mod wmf;
pub mod wordcount;
pub mod xml_parse;
