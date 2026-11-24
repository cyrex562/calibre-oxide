//! Port of `old_src/src/calibre/utils/open_with/` (issue #72):
//! finding installed applications that can open a given file type,
//! and building a command line to launch one on it ("Open With..."
//! in calibre's GUI).
//!
//! # Scope
//!
//! [`linux`] (`linux.py`) is a real, tested port -- freedesktop.org
//! `.desktop` file discovery needs only the filesystem, no native
//! toolkit.
//!
//! `osx.py` and `windows.py` are **not ported**: both are, at their
//! core, bindings to a native OS application registry --
//! `osx.py` shells out to macOS's `mdfind`/Launch Services and parses
//! `.app` bundle `Info.plist` files; `windows.py` calls into
//! `calibre_extensions.winutil` (a native Win32 DLL enumerating the
//! registry's `HKEY_CLASSES_ROOT`/App Paths) and Qt (`QPixmap`) for
//! icon extraction. Neither has a meaningful Rust equivalent to write
//! *and verify* on this project's Linux-only development environment
//! -- there is no macOS or Windows host to run either against, and
//! both are thin enough over native, OS-specific APIs that a blind,
//! untested port would carry a real risk of being silently wrong.
//! Matches the same scoping already applied to `utils::windows`/
//! `utils::winreg` (issues #78/#79) and the network-storage
//! fault-tolerance issues (#262-265): real native-platform
//! integration work, left for whoever can actually run and verify it
//! on the target OS.

pub mod linux;
