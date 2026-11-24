// The one place that maps the app's icon names to Lucide components.
//
// Names are the keys that `actions.ts` (`icon:`), the toolbar layout and
// a few components already use, so call sites barely change. Lucide is
// ISC licensed; the icons are line art in `currentColor`, so they follow
// the theme without any filter. See docs/UI_DESIGN.md §2.4.

import {
  Archive, ArrowDown, ArrowRightLeft, ArrowUpDown, BookOpen, BookPlus, BookText,
  BookmarkCheck, Braces, Building2, ChevronDown, CircleAlert, CircleHelp, ClipboardCheck,
  CloudDownload, Columns3, Copy, Ellipsis, Eye, ExternalLink, FileSearch,
  FolderInput, FolderOpen, FolderSearch, Highlighter, Image, Languages,
  LayoutGrid, Layers, Library, ListFilter, ListTodo, Mail, Merge,
  Minus, Newspaper, PackageOpen, PanelLeft, RefreshCw, Save, ScanSearch,
  Search, Settings, Shuffle, SlidersHorizontal, Sparkles, Star, SquarePen,
  Tags, Trash2, Undo2, User, Wrench, X,
} from "lucide-vue-next";
import type { Component } from "vue";

export const ICONS: Record<string, Component> = {
  // Toolbar: getting books in
  add_book: BookPlus,
  tb_folder: FolderOpen,
  news: Newspaper,
  // Toolbar: metadata
  edit_input: SquarePen,
  "download-metadata": CloudDownload,
  default_cover: Image,
  merge_books: Merge,
  tags: Tags,
  template_funcs: Braces,
  // Toolbar: conversion and editing
  convert: ArrowRightLeft,
  polish: Sparkles,
  tweak: Wrench,
  "unpack-book": PackageOpen,
  sync: RefreshCw,
  // Toolbar: reading
  view: BookOpen,
  "external-link": ExternalLink,
  quickview: Eye,
  similar: ScanSearch,
  // Toolbar: getting books out
  save: Save,
  mail: Mail,
  catalog: BookText,
  bookshelf: Archive,
  copy_to_library: FolderInput,
  // Toolbar: removal
  remove_books: Trash2,
  "edit-undo": Undo2,
  // Toolbar: right-hand cluster
  lt: Library,
  config: Settings,
  help: CircleHelp,
  // Search bar
  search: Search,
  gear: SlidersHorizontal,
  fts: FileSearch,
  folder_saved_search: FolderSearch,
  sort: ArrowUpDown,
  minus: Minus,
  "arrow-down": ArrowDown,
  dialog_error: CircleAlert,
  "window-close": X,
  vl: ListFilter,
  // View switching and layout
  grid: LayoutGrid,
  layout: PanelLeft,
  "h-ellipsis": Ellipsis,
  "chevron-down": ChevronDown,
  // Library tools
  reports: ClipboardCheck,
  merge: Copy,
  column: Columns3,
  highlight: Highlighter,
  random: Shuffle,
  jobs: ListTodo,
  marked: BookmarkCheck,
  // Category browser
  user_profile: User,
  series: Layers,
  publisher: Building2,
  languages: Languages,
  rating: Star,
};

/** The Lucide component for an icon name, or undefined if unmapped. */
export function iconComponent(name: string | undefined | null): Component | undefined {
  return name ? ICONS[name] : undefined;
}
