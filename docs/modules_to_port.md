# Modules to port

## src/calibre

### ai

- [x] __init__.py
- [ ] config.py
- [x] prefs.py
- [x] utils.py

#### github

- [x] __init__.py
- [x] backend.py
- [ ] config.py

#### google

- [x] __init__.py
- [x] backend.py
- [ ] config.py

#### lm_studio

- [x] __init__.py
- [x] backend.py
- [ ] config.py

#### ollama

- [x] __init__.py
- [x] backend.py
- [ ] config.py

#### open_router

- [x] __init__.py
- [x] backend.py
- [ ] config.py

#### openai

- [x] __init__.py
- [x] backend.py
- [ ] config.py

### customize

- [x] __init__.py
- [x] builtins.py
- [x] conversion.py
- [x] profiles.py
- [x] ui.py
- [x] zipplugin.py

### db

- [ ] __init__.py
- [x] adding.py
- [x] annotations.py
- [x] backend.py
- [x] backup.py
- [x] cache.py
- [x] categories.py
- [x] constants.py
- [x] copy_to_library.py
- [x] covers.py
- [x] errors.py
- [x] fields.py
- [x] lazy.py
- [x] legacy.py
- [x] listeners.py
- [x] locking.py
- [x] restore.py
- [x] schema_upgrades.py
- [x] search.py
- [ ] sqlite_extension.cpp
- [x] tables.py
- [x] utils.py
- [x] view.py
- [x] write.py

#### cli

- [ ] __init__.py
- [x] `cmd_add_custom_column.py` -> `crates/calibre_db/src/cli/cmd_add_custom_column.rs`
- [x] cmd_add_format.py
- [x] cmd_add.py
- [x] `cmd_backup_metadata.py` -> `crates/calibre_db/src/cli/cmd_backup_metadata.rs`
- [x] cmd_catalog.py
- [x] `cmd_check_library.py` (Partial/Skeleton) -> `crates/calibre_db/src/cli/cmd_check_library.rs`
- [x] cmd_clone.py
- [x] cmd_custom_columns.py
- [x] cmd_embed_metadata.py
- [x] cmd_export.py
- [x] cmd_fits_index.py
- [x] cmd_fits_search.py
- [x] cmd_list_categories.py
- [x] cmd_list.py
- [x] `cmd_remove_custom_column.py` -> `crates/calibre_db/src/cli/cmd_remove_custom_column.rs`
- [x] cmd_remove_format.py
- [x] cmd_restore_database.py
- [x] cmd_saved_searches.py
- [x] cmd_search.py
- [x] cmd_set_custom.py
- [x] cmd_set_metadata.py
- [x] cmd_show_metadata.py
- [x] cmd_switch.py
- [x] main.py
- [ ] tests.py
- [x] utils.py

#### fts

- [x] __init__.py
- [x] connect.py
- [x] pool.py
- [x] schema_upgrade.py
- [x] text.py

#### notes

- [x] __init__.py
- [x] connect.py
- [x] exim.py
- [x] schema_upgrade.py

### devices

- [x] __init__.py
- [x] cli.py
- [x] errors.py
- [x] interface.py
- [x] mime.py
- [x] misc.py
- [x] scanner.py
- [x] udisks.py
- [x] utils.py
- [x] winusb.py

#### android

- [x] __init__.py
- [x] driver.py

#### folder_device

- [x] __init__.py
- [x] driver.py

#### kindle

- [x] __init__.py
- [x] driver.py
- [x] apnx.py
- [x] bookmark.py

##### apnx_page_generator

- [ ] __init__.py
- [ ] i_page_generator.py
- [ ] page_group.py
- [ ] page_number_type.py
- [ ] pages.py

###### generators

- [ ] __init__.py
- [ ] accurate_page_generator.py
- [ ] exact_page_generator.py
- [ ] fast_page_generator.py
- [ ] pagebreak_page_generator.py

#### kobo

- [ ] __init__.py
- [ ] driver.py
- [ ] bookmark.py
- [ ] books.py
- [ ] db.py
- [ ] kobotouch_config.py

#### libusb

- [ ] libusb.c

#### mtp

- [ ] __init__.py
- [ ] base.py
- [ ] books.py
- [ ] defaults.py
- [ ] driver.py
- [ ] filesystem_cache.py
- [ ] test.py

##### unix

- [ ] __init__.py
- [ ] devices.c
- [ ] devices.h
- [ ] driver.py
- [ ] libmtp.c
- [ ] sysfs.py

###### upstream

- [ ] device-flags.h
- [ ] music-players.h
- [ ] update.py

##### windows

- [ ] __init__.py
- [ ] content_enumeration.cpp
- [ ] device_enumeration.cpp
- [ ] device.cpp
- [ ] driver.py
- [ ] global.h
- [ ] wpd.cpp

#### nook

- [x] __init__.py
- [x] driver.py

#### smart_device_app

- [x] __init__.py
- [x] driver.py

#### usbms

- [x] __init__.py
- [x] books.py
- [x] cli.py
- [x] device.py
- [x] deviceconfig.py
- [x] driver.py

#### usbobserver

- [ ] usbobserver.c

#### userdefined

- [x] __init__.py
- [x] driver.py

### ebooks

- [ ] __init__.py
- [ ] BeautifulSoup.py
- [ ] chardet.py
- [ ] constants.py
- [ ] covers.py
- [ ] css_transform_rules.py
- [ ] html_entities.c
- [ ] html_entities.h
- [ ] html_entities.py
- [ ] html_transform_rules.py
- [ ] hyphenate.py
- [ ] render_html.py
- [ ] tweak.py
- [ ] uchardet.c

#### azw4

- [ ] __init__.py
- [ ] reader.py

#### chm

- [ ] __init__.py
- [ ] reader.py
- [ ] metadata.py

#### comic

- [ ] __init__.py
- [ ] input.py


#### conversion

- [ ] __init__.py
- [x] archives.py
- [ ] cli.py
- [ ] config.py
- [x] plumber.py
- [x] preprocess.py
- [x] search_replace.py
- [x] utils.py

##### plugins

- [ ] __init__.py
- [x] azw4_input.py (Wrapper)
- [x] chm_input.py (Regex/Placeholder)
- [x] comic_input.py
- [x] djvu_input.py (Placeholder)
- [x] docx_input.py
- [x] docx_output.py (Stub)
- [x] epub_input.py
- [x] epub_output.py
- [x] fb2_input.py
- [x] fb2_output.py
- [x] html_input.py
- [x] html_output.py
- [x] htmlz_input.py
- [x] htmlz_output.py
- [x] lit_input.py
- [x] lit_output.py
- [x] lrf_input.py
- [x] mobi_input.py
- [x] mobi_output.py
- [x] `odt_input.py` -> `input/odt_input.rs`
- [x] `odt_output.py` -> `output/odt_output.rs`
- [x] oeb_output.py
- [x] pdb_input.py
- [x] pdb_output.py
- [x] `pdf_input.py` -> `input/pdf_input.rs`
- [x] `pdf_output.py` -> `output/pdf_output.rs`
- [x] pml_input.py
- [x] pml_output.py
- [x] rb_input.py
- [x] rb_output.py
- [x] recipe_input.py (Placeholder)
- [x] rtf_input.py
- [x] snb_input.py
- [x] snb_output.py
- [x] tcr_input.py
- [x] `tcr_output.py` -> `output/tcr_output.rs`
- [x] txt_input.py
- [x] txt_output.py
- [x] zip_input.py
- [x] rar_input.py (Placeholder)

#### djvu

- [ ] __init__.py
- [ ] bzzdecoder.c
- [ ] djvu.py
- [ ] djvubzzdec.py

#### docx

- [ ] __init__.py
- [ ] block_styles.py
- [ ] char_styles.py
- [ ] cleanup.py
- [ ] container.py
- [ ] dump.py
- [ ] fields.py
- [ ] fonts.py
- [ ] footnotes.py
- [ ] images.py
- [ ] index.py
- [ ] lcid.py
- [ ] names.py
- [ ] numbering.py
- [ ] settings.py
- [ ] styles.py
- [ ] tables.py
- [ ] theme.py
- [ ] to_html.py
- [ ] toc.py

##### writer

- [ ] __init__.py
- [ ] container.py
- [ ] fonts.py
- [ ] from_html.py
- [ ] images.py
- [ ] links.py
- [ ] lists.py
- [ ] styles.py
- [ ] tables.py
- [ ] TODO
- [ ] utils.py

#### epub

- [ ] __init__.py
- [ ] pages.py
- [ ] periodical.py

##### cfi

- [ ] __init__.py
- [ ] epublfi.ebnf
- [ ] parse.py
- [ ] tests.py

#### fb2

- [ ] __init__.py
- [ ] fb2ml.py

#### html

- [ ] __init__.py
- [ ] input.py
- [ ] meta.py
- [ ] to_zip.py

#### htmlz

- [ ] __init__.py
- [ ] oeb2html.py

#### iterator

- [ ] __init__.py

#### lit

- [ ] __init__.py
- [ ] lzx.py
- [ ] mssha1.py
- [ ] reader.py
- [ ] writer.py

##### maps

- [ ] __init__.py
- [ ] html.py
- [ ] opf.py

#### lrf

- [ ] __init__.py
- [ ] fonts.py
- [ ] input.py
- [ ] lrfparser.py
- [ ] meta.py
- [ ] objects.py
- [ ] tags.py

##### html

- [ ] __init__.py
- [ ] color_map.py
- [ ] convert_from.py
- [ ] table.py

###### demo

- [ ] a.png
- [ ] demo.html
- [ ] large.jpg
- [ ] medium.jpg
- [ ] small.jpg

##### lrs

- [ ] __init__.py
- [ ] convert_from.py

##### pylrs

- [ ] __init__.py
- [ ] elements.py
- [ ] pylrf.py
- [ ] pylrfopt.py
- [ ] pylrs.py

#### metadata

- [x] __init__.py
- [x] archive.py
- [x] author_mapper.py
- [x] cli.py (Replaced by ebook-meta.rs)
- [x] docx.py
- [x] epub.py
- [x] ereader.py
- [x] extz.py
- [x] fb2.py
- [x] haodoo.py
- [x] html.py
- [x] imp.py
- [x] kdl.py (Skipped - specific scraper)
- [x] kfx.py
- [x] lit.py
- [x] lrx.py
- [x] mobi.py
- [x] odt.py
- [x] opf_2_to_3.py (Merged into opf.rs/cli)
- [x] opf.py
- [x] opf2.py (Merged into opf.rs)
- [x] opf3_test.py (Covered by opf_refinement_test.rs)
- [x] opf3.py (Merged into opf.rs)
- [x] pdb.py
- [x] pdf.py
- [x] plucker.py
- [x] pml.py
- [ ] rar.py
- [x] rb.rs
- [x] rtf.rs
- [x] search_internet.py
- [x] snb.py
- [x] tag_mapper.py
- [x] test_author_sort.py
- [x] toc.py
- [x] topaz.py
- [x] txt.py
- [x] utils.py
- [ ] worker.py
- [x] xmp.py (Unverified - Test Ignored)
- [x] zip.py
- [x] rar.py (Stub)

#### ebooks/metadata
- [x] chm.py (Regex-based port)
- [x] azw4.py (PDF wrapper port)

#### mobi

- [ ] __init__.py
- [ ] huffcdic.py
- [ ] langcodes.py
- [ ] mobiml.py
- [ ] tbs_periodicals.rst
- [ ] tweak.py
- [ ] utils.py

##### debug

- [ ] __init__.py
- [ ] containers.py
- [ ] headers.py
- [ ] index.py
- [ ] main.py
- [ ] mobi6.py
- [ ] mobi8.py

##### reader

- [ ] __init__.py
- [ ] containers.py
- [ ] headers.py
- [ ] index.py
- [ ] markup.py
- [ ] mobi6.py
- [ ] mobi8.py
- [ ] ncx.py

##### writer2

- [ ] __init__.py
- [ ] indexer.py
- [ ] main.py
- [ ] resources.py
- [ ] serializer.py

##### writer8

- [ ] __init__.py
- [ ] cleanup.py
- [ ] exth.py
- [ ] header.py
- [ ] index.py
- [ ] main.py
- [ ] mobi.py
- [ ] skeleton.py
- [ ] tbs.py
- [ ] toc.py

#### odt

- [ ] __init__.py
- [ ] input.py

#### oeb

- [x] __init__.py
- [x] base.py (Constants, Metadata, Container ported)
- [x] normalize_css.py (DEFAULTS and edge normalization ported)
- [x] parse_utils.py (XML Namespace helpers, iterlinks, xml2text ported)
- [x] reader.py (OEBReader and OEBBook structure ported)
- [x] stylizer.py (Basic Stylizer/Style struct with inheritance ported)
- [x] writer.py (OEBWriter for OPF generation ported)

##### display

- [ ] __init__.py
- [ ] webview.py

##### iterator

- [ ] __init__.py
- [ ] book.py
- [ ] bookmarks.py
- [ ] spine.py

##### polish

- [ ] __init__.py
- [ ] cascade.py
- [ ] container.py
- [ ] cover.py
- [ ] create.py
- [ ] css.py
- [ ] download.py
- [ ] embed.py
- [ ] errors.py
- [ ] fonts.py
- [ ] hyphenation.py
- [ ] images.py
- [ ] import_book.py
- [ ] jacket.py
- [ ] kepubify.py
- [ ] main.py
- [ ] opf.py
- [ ] parsing.py
- [ ] pretty.py
- [ ] replace.py
- [ ] report.py
- [ ] spell.py
- [ ] split.py
- [ ] stats.py
- [ ] subset.py
- [ ] toc.py
- [ ] tts.py
- [ ] upgrade.py
- [ ] utils.py

###### check

- [ ] __init__.py
- [ ] base.py
- [ ] css.py
- [ ] fonts.py
- [ ] images.py
- [ ] links.py
- [ ] main.py
- [ ] opf.py
- [ ] parsing.py

##### transforms

- [ ] __init__.py
- [ ] alt_text.py
- [ ] cover.py
- [ ] data_url.py
- [ ] embed_fonts.py
- [ ] filenames.py
- [ ] flatcss.py
- [ ] guide.py
- [ ] htmltoc.py
- [ ] jacket.py
- [ ] linearize_tables.py
- [ ] manglecase.py
- [ ] metadata.py
- [ ] page_margin.py
- [ ] rasterize.py
- [ ] rescale.py
- [ ] split.py
- [ ] structure.py
- [ ] subset.py
- [ ] trimmanifest.py
- [ ] unsmarten.py

#### pdb

- [ ] __init__.py
- [ ] formatreader.py
- [ ] formatwriter.py
- [ ] header.py

##### ereader

- [ ] __init__.py
- [ ] inspector.py
- [ ] reader.py
- [ ] reader132.py
- [ ] reader202.py
- [ ] writer.py

##### pdf

- [ ] __init__.py
- [ ] reader.py

##### plucker

- [ ] __init__.py
- [ ] reader.py

##### ztxt

- [ ] __init__.py
- [ ] formatreader.py
- [ ] formatwriter.py
- [ ] header.py

#### pdf

- [ ] __init__.py
- [ ] develop.py
- [ ] html_writer.py
- [ ] image_writer.py
- [ ] pdftohtml.py
- [ ] reflow.py
- [ ] utils.h

##### render

- [ ] __init__.py
- [ ] common.py
- [ ] fonts.py
- [ ] gradients.py
- [ ] graphics.py
- [ ] links.py
- [ ] serialize.py

#### pml

- [ ] __init__.py
- [ ] pmlconverter.py
- [ ] pmlml.py

#### rb

- [ ] __init__.py
- [ ] rbml.py
- [ ] reader.py
- [ ] writer.py

#### readability

- [ ] __init__.py
- [ ] cleaners.py
- [ ] debug.py
- [ ] htmls.py
- [ ] readability.py

#### rtf

- [ ] __init__.py
- [ ] input.py
- [ ] preprocess.py
- [ ] rtfml.py

#### rtf2xml

- [ ] __init__.py
- [ ] add_brackets.py
- [ ] body_styles.py
- [ ] border_parse.py
- [ ] char_set.py
- [ ] check_brackets.py
- [ ] check_encoding.py
- [ ] colors.py
- [ ] combine_borders.py
- [ ] configure_txt.py
- [ ] convert_to_tags.py
- [ ] copy.py
- [ ] default_encoding.py
- [ ] delete_info.py
- [ ] field_strings.py
- [ ] fields_large.py
- [ ] fields_small.py
- [ ] fonts.py
- [ ] footnote.py
- [ ] get_char_map.py
- [ ] get_options.py
- [ ] group_borders.py
- [ ] group_styles.py
- [ ] header.py
- [ ] headings_to_sections.py
- [ ] hex_2_utf8.py
- [ ] info.py
- [ ] inline.py
- [ ] line_endings.py
- [ ] list_numbers.py
- [ ] list_table.py
- [ ] make_lists.py
- [ ] old_rtf.py
- [ ] options_trem.py
- [ ] output.py
- [ ] override_table.py
- [ ] paragraph_def.py
- [ ] paragraphs.py
- [ ] ParseRtf.py
- [ ] pict.py
- [ ] preamble_div.py
- [ ] preamble_rest.py
- [ ] process_tokens.py
- [ ] replace_illegals.py
- [ ] sections.py
- [ ] styles.py
- [ ] table_info.py
- [ ] table.py
- [ ] tokenize.py

#### snb

- [ ] __init__.py
- [ ] snbfile.py
- [ ] snbml.py

#### tcr

- [ ] __init__.py

#### textile

- [ ] __init__.py
- [ ] functions.py
- [ ] unsmarten.py

#### txt

- [ ] __init__.py
- [ ] markdownml.py
- [ ] newlines.py
- [ ] processor.py
- [ ] textileml.py
- [ ] txtml.py

#### unihandecode

- [ ] __init__.py
- [ ] jacodepoints.py
- [ ] jadecoder.py
- [ ] krcodepoints.py
- [ ] krdecoder.py
- [ ] unicodepoints.py
- [ ] unidecoder.py
- [ ] vncodepoints.py
- [ ] vndecoder.py
- [ ] zhcodepoints.py

### gui2

- [ ] add.py
- [ ] add_filters.py
- [ ] author_mapper.py
- [ ] auto_add.py
- [ ] bars.py
- [ ] book_details.py
- [ ] central.py
- [ ] changes.py
- [ ] chat_widget.py
- [ ] comments_editor.py
- [ ] complete2.py
- [ ] covers.py
- [ ] cover_flow.py
- [ ] css_transform_rules.py
- [ ] custom_column_widgets.py
- [ ] device.py
- [ ] dnd.py
- [ ] ebook_download.py
- [ ] email.py
- [ ] extra_files_watcher.py
- [ ] filename_pattern.ui
- [ ] flow_toolbar.py
- [ ] font_family_chooser.py
- [ ] geometry.py
- [ ] gestures.py
- [ ] html_transform_rules.py
- [ ] icon_theme.py
- [ ] image_popup.py
- [ ] init.py
- [ ] jobs.py
- [ ] job_indicator.py
- [ ] keyboard.py
- [ ] languages.py
- [ ] layout.py
- [ ] layout_menu.py
- [ ] linux_file_dialogs.py
- [ ] listener.py
- [ ] llm.py
- [ ] main.py
- [ ] main_window.py
- [ ] markdown_editor.py
- [ ] markdown_syntax_highlighter.py
- [ ] momentum_scroll.py
- [ ] notify.py
- [ ] open_with.py
- [ ] palette.py
- [ ] pin_columns.py
- [ ] proceed.py
- [ ] publisher_mapper.py
- [ ] pyqt6_compat.py
- [ ] qt_file_dialogs.py
- [ ] save.py
- [ ] search_box.py
- [ ] search_restriction_mixin.py
- [ ] series_mapper.py
- [ ] splash_screen.py
- [ ] tag_mapper.py
- [ ] threaded_jobs.py
- [ ] throbber.py
- [ ] tools.py
- [ ] trash.py
- [ ] ui.py
- [ ] update.py
- [ ] webengine.py
- [ ] widgets.py
- [ ] widgets2.py
- [ ] win_file_dialogs.py
- [ ] __init__.py

#### actions

- [ ] add.py
- [ ] add_to_library.py
- [ ] all_actions.py
- [ ] annotate.py
- [ ] author_mapper.py
- [ ] auto_scroll.py
- [ ] booklist_context_menu.py
- [ ] browse_annots.py
- [ ] browse_notes.py
- [ ] catalog.py
- [ ] choose_library.py
- [ ] column_tooltips.py
- [ ] convert.py
- [ ] copy_to_library.py
- [ ] delete.py
- [ ] device.py
- [ ] edit_collections.py
- [ ] edit_metadata.py
- [ ] embed.py
- [ ] fetch_news.py
- [ ] fts.py
- [ ] help.py
- [ ] layout_actions.py
- [ ] llm_book.py
- [ ] manage_categories.py
- [ ] mark_books.py
- [ ] match_books.py
- [ ] next_match.py
- [ ] open.py
- [ ] plugin_updates.py
- [ ] polish.py
- [ ] preferences.py
- [ ] random.py
- [ ] restart.py
- [ ] saved_searches.py
- [ ] save_to_disk.py
- [ ] show_book_details.py
- [ ] show_quickview.py
- [ ] show_stored_templates.py
- [ ] show_template_tester.py
- [ ] similar_books.py
- [ ] sort.py
- [ ] store.py
- [ ] tag_mapper.py
- [ ] toc_edit.py
- [ ] tweak_epub.py
- [ ] unpack_book.py
- [ ] view.py
- [ ] virtual_library.py
- [ ] __init__.py

#### catalog

- [ ] catalog_bibtex.py
- [ ] catalog_bibtex.ui
- [ ] catalog_csv_xml.py
- [ ] catalog_epub_mobi.py
- [ ] catalog_epub_mobi.ui
- [ ] catalog_tab_template.ui
- [ ] __init__.py

#### convert

- [ ] azw3_output.py
- [ ] azw3_output.ui
- [ ] bulk.py
- [ ] comic_input.py
- [ ] comic_input.ui
- [ ] debug.py
- [ ] debug.ui
- [ ] docx_input.py
- [ ] docx_input.ui
- [ ] docx_output.py
- [ ] epub_output.py
- [ ] epub_output.ui
- [ ] fb2_input.py
- [ ] fb2_input.ui
- [ ] fb2_output.py
- [ ] fb2_output.ui
- [ ] font_key.py
- [ ] font_key.ui
- [ ] gui_conversion.py
- [ ] heuristics.py
- [ ] heuristics.ui
- [ ] htmlz_output.py
- [ ] htmlz_output.ui
- [ ] kepub_output.py
- [ ] kepub_output.ui
- [ ] look_and_feel.py
- [ ] look_and_feel.ui
- [ ] lrf_output.py
- [ ] lrf_output.ui
- [ ] metadata.py
- [ ] metadata.ui
- [ ] mobi_output.py
- [ ] mobi_output.ui
- [ ] page_setup.py
- [ ] page_setup.ui
- [ ] pdb_output.py
- [ ] pdb_output.ui
- [ ] pdf_input.py
- [ ] pdf_input.ui
- [ ] pdf_output.py
- [ ] pdf_output.ui
- [ ] pmlz_output.ui
- [ ] pml_output.py
- [ ] rb_output.py
- [ ] rb_output.ui
- [ ] regex_builder.py
- [ ] regex_builder.ui
- [ ] rtf_input.py
- [ ] rtf_input.ui
- [ ] search_and_replace.py
- [ ] search_and_replace.ui
- [ ] single.py
- [ ] snb_output.py
- [ ] snb_output.ui
- [ ] structure_detection.py
- [ ] structure_detection.ui
- [ ] toc.py
- [ ] toc.ui
- [ ] txtz_output.py
- [ ] txt_input.py
- [ ] txt_input.ui
- [ ] txt_output.py
- [ ] txt_output.ui
- [ ] xpath_wizard.py
- [ ] xpath_wizard.ui
- [ ] __init__.py

#### device_drivers

- [ ] configwidget.py
- [ ] configwidget.ui
- [ ] mtp_config.py
- [ ] mtp_folder_browser.py
- [ ] tabbed_device_config.py
- [ ] __init__.py

#### dialogs

- [ ] add_empty_book.py
- [ ] add_from_isbn.py
- [ ] authors_edit.py
- [ ] book_info.py
- [ ] catalog.py
- [ ] catalog.ui
- [ ] check_library.py
- [ ] choose_format.py
- [ ] choose_format_device.py
- [ ] choose_format_device.ui
- [ ] choose_library.py
- [ ] choose_library.ui
- [ ] choose_plugin_toolbars.py
- [ ] comments_dialog.py
- [ ] confirm_delete.py
- [ ] confirm_delete_location.py
- [ ] confirm_merge.py
- [ ] connect_to_folder.py
- [ ] conversion_error.py
- [ ] conversion_error.ui
- [ ] custom_recipes.py
- [ ] data_files_manager.py
- [ ] delete_matching_from_device.py
- [ ] delete_matching_from_device.ui
- [ ] device_category_editor.py
- [ ] device_category_editor.ui
- [ ] drm_error.py
- [ ] drm_error.ui
- [ ] duplicates.py
- [ ] edit_authors_dialog.py
- [ ] edit_authors_dialog.ui
- [ ] edit_category_notes.py
- [ ] enum_values_edit.py
- [ ] exim.py
- [ ] ff_doc_editor.py
- [ ] jobs.ui
- [ ] llm_book.py
- [ ] match_books.py
- [ ] match_books.ui
- [ ] message_box.py
- [ ] metadata_bulk.py
- [ ] metadata_bulk.ui
- [ ] multisort.py
- [ ] opml.py
- [ ] palette.py
- [ ] password.py
- [ ] password.ui
- [ ] plugin_updater.py
- [ ] progress.py
- [ ] quickview.py
- [ ] quickview.ui
- [ ] restore_library.py
- [ ] saved_search_editor.py
- [ ] scheduler.py
- [ ] search.py
- [ ] select_formats.py
- [ ] show_category_note.py
- [ ] smartdevice.py
- [ ] smartdevice.ui
- [ ] tag_categories.py
- [ ] tag_categories.ui
- [ ] tag_editor.py
- [ ] tag_editor.ui
- [ ] tag_list_editor.py
- [ ] tag_list_editor.ui
- [ ] tag_list_editor_table_widget.py
- [ ] template_dialog.py
- [ ] template_dialog.ui
- [ ] template_dialog_box_layout.py
- [ ] template_dialog_code_widget.py
- [ ] template_general_info.py
- [ ] template_line_editor.py
- [ ] trim_image.py
- [ ] __init__.py

#### fts

- [ ] dialog.py
- [ ] scan.py
- [ ] search.py
- [ ] utils.py
- [ ] __init__.py

#### library

- [ ] alternate_views.py
- [ ] annotations.py
- [ ] bookshelf_view.py
- [ ] caches.py
- [ ] delegates.py
- [ ] models.py
- [ ] notes.py
- [ ] views.py
- [ ] __init__.py

#### lrf_renderer

- [ ] bookview.py
- [ ] config.ui
- [ ] document.py
- [ ] main.py
- [ ] main.ui
- [ ] text.py
- [ ] __init__.py

#### metadata

- [ ] basic_widgets.py
- [ ] bulk_download.py
- [ ] config.py
- [ ] diff.py
- [ ] pdf_covers.py
- [ ] single.py
- [ ] single_download.py
- [ ] __init__.py

#### pictureflow

- [ ] pictureflow.cpp
- [ ] pictureflow.h
- [ ] pictureflow.sip

#### preferences

- [ ] device_debug.py
- [ ] device_user_defined.py
- [ ] email.ui
- [ ] emailp.py
- [ ] history.py
- [ ] ignored_devices.py
- [ ] keyboard.py
- [ ] look_feel.py
- [ ] look_feel.ui
- [ ] main.py
- [ ] metadata_sources.py
- [ ] metadata_sources.ui
- [ ] misc.py
- [ ] misc.ui
- [ ] plugboard.py
- [ ] plugboard.ui
- [ ] plugins.py
- [ ] plugins.ui
- [ ] save_template.py
- [ ] save_template.ui
- [ ] saving.py
- [ ] saving.ui
- [ ] search.py
- [ ] search.ui
- [ ] sending.py
- [ ] sending.ui
- [ ] server.py
- [ ] template_functions.py
- [ ] template_functions.ui
- [ ] texture_chooser.py
- [ ] toolbar.py
- [ ] toolbar.ui
- [ ] tweaks.py
- [ ] __init__.py

##### look_feel_tabs

- [ ] bookshelf_view.py
- [ ] bookshelf_view.ui
- [ ] book_details.py
- [ ] book_details.ui
- [ ] cover_grid.py
- [ ] cover_grid.ui
- [ ] cover_view.py
- [ ] cover_view.ui
- [ ] edit_metadata.py
- [ ] edit_metadata.ui
- [ ] main_interface.py
- [ ] main_interface.ui
- [ ] quickview.py
- [ ] quickview.ui
- [ ] tb_display.py
- [ ] tb_display.ui
- [ ] tb_hierarchy.py
- [ ] tb_hierarchy.ui
- [ ] tb_icon_rules.py
- [ ] tb_icon_rules.ui
- [ ] tb_partitioning.py
- [ ] tb_partitioning.ui
- [ ] __init__.py

#### progress_indicator

- [ ] CalibreStyle.cpp
- [ ] QProgressIndicator.cpp
- [ ] QProgressIndicator.h
- [ ] QProgressIndicator.sip
- [ ] __init__.py

#### store

- [ ] amazon_base.py
- [ ] amazon_live.py
- [ ] basic_config.py
- [ ] basic_config_widget.ui
- [ ] declined.txt
- [ ] loader.py
- [ ] opensearch_store.py
- [ ] search_result.py
- [ ] web_store.py
- [ ] web_store_dialog.py
- [ ] __init__.py

##### config

- [ ] store.py
- [ ] __init__.py

###### chooser

- [ ] chooser_dialog.py
- [ ] chooser_widget.py
- [ ] chooser_widget.ui
- [ ] models.py
- [ ] results_view.py
- [ ] __init__.py

###### search

- [ ] search_widget.py
- [ ] search_widget.ui
- [ ] __init__.py

##### search

- [ ] adv_search_builder.py
- [ ] adv_search_builder.ui
- [ ] download_thread.py
- [ ] models.py
- [ ] results_view.py
- [ ] search.py
- [ ] search.ui
- [ ] __init__.py

##### stores

- [ ] baen_webscription_plugin.py
- [ ] beam_ebooks_de_plugin.py
- [ ] biblio_plugin.py
- [ ] bn_plugin.py
- [ ] bubok_portugal_plugin.py
- [ ] bubok_publishing_plugin.py
- [ ] chitanka_plugin.py
- [ ] ebookpoint_plugin.py
- [ ] ebooksgratuits_plugin.py
- [ ] ebookshoppe_uk_plugin.py
- [ ] ebooks_com_plugin.py
- [ ] ebook_nl_plugin.py
- [ ] empik_plugin.py
- [ ] feedbooks_plugin.py
- [ ] google_books_plugin.py
- [ ] gutenberg_plugin.py
- [ ] kobo_plugin.py
- [ ] legimi_plugin.py
- [ ] libri_de_plugin.py
- [ ] litres_plugin.py
- [ ] manybooks_plugin.py
- [ ] mills_boon_uk_plugin.py
- [ ] nexto_plugin.py
- [ ] ozon_ru_plugin.py
- [ ] pragmatic_bookshelf_plugin.py
- [ ] publio_plugin.py
- [ ] rw2010_plugin.py
- [ ] smashwords_plugin.py
- [ ] swiatebookow_plugin.py
- [ ] virtualo_plugin.py
- [ ] weightless_books_plugin.py
- [ ] woblink_plugin.py
- [ ] wolnelektury_plugin.py
- [ ] __init__.py

###### mobileread

- [ ] adv_search_builder.py
- [ ] adv_search_builder.ui
- [ ] cache_progress_dialog.py
- [ ] cache_progress_dialog.ui
- [ ] cache_update_thread.py
- [ ] mobileread_plugin.py
- [ ] models.py
- [ ] store_dialog.py
- [ ] store_dialog.ui
- [ ] __init__.py

#### tag_browser

- [ ] model.py
- [ ] ui.py
- [ ] view.py
- [ ] __init__.py

#### toc

- [ ] location.py
- [ ] main.py
- [ ] __init__.py

#### tts

- [ ] config.py
- [ ] develop.py
- [ ] download.py
- [ ] manager.py
- [ ] piper.py
- [ ] qt.py
- [ ] speechd.py
- [ ] types.py
- [ ] __init__.py

#### tweak_book

- [ ] boss.py
- [ ] char_select.py
- [ ] check.py
- [ ] check_links.py
- [ ] download.py
- [ ] file_list.py
- [ ] function_replace.py
- [ ] job.py
- [ ] jump_to_class.py
- [ ] live_css.py
- [ ] main.py
- [ ] manage_fonts.py
- [ ] plugin.py
- [ ] polish.py
- [ ] preferences.py
- [ ] preview.py
- [ ] reports.py
- [ ] save.py
- [ ] search.py
- [ ] spell.py
- [ ] templates.py
- [ ] text_search.py
- [ ] toc.py
- [ ] tts.py
- [ ] ui.py
- [ ] undo.py
- [ ] widgets.py
- [ ] __init__.py

##### completion

- [ ] basic.py
- [ ] popup.py
- [ ] utils.py
- [ ] worker.py
- [ ] __init__.py

##### diff

- [ ] highlight.py
- [ ] main.py
- [ ] view.py
- [ ] _patiencediff_c.c
- [ ] __init__.py

##### editor

- [ ] canvas.py
- [ ] comments.py
- [ ] help.py
- [ ] image.py
- [ ] insert_resource.py
- [ ] snippets.py
- [ ] text.py
- [ ] themes.py
- [ ] widget.py
- [ ] __init__.py

###### smarts

- [ ] css.py
- [ ] html.py
- [ ] python.py
- [ ] utils.py
- [ ] __init__.py

###### syntax

- [ ] base.py
- [ ] css.py
- [ ] html.c
- [ ] html.py
- [ ] javascript.py
- [ ] pygments_highlighter.py
- [ ] python.py
- [ ] utils.py
- [ ] xml.py
- [ ] __init__.py

#### viewer

- [ ] annotations.py
- [ ] bookmarks.py
- [ ] config.py
- [ ] control_sleep.py
- [ ] convert_book.py
- [ ] highlights.py
- [ ] integration.py
- [ ] llm.py
- [ ] lookup.py
- [ ] main.py
- [ ] overlay.py
- [ ] printing.py
- [ ] search.py
- [ ] shortcuts.py
- [ ] toc.py
- [ ] toolbars.py
- [ ] tts.py
- [ ] ui.py
- [ ] web_view.py
- [ ] widgets.py
- [ ] __init__.py

#### wizard

- [ ] device.ui
- [ ] finish.ui
- [ ] kindle.ui
- [ ] library.ui
- [ ] send_email.py
- [ ] send_email.ui
- [ ] stanza.ui
- [ ] __init__.py

### headless

- [ ] CMakeLists.txt
- [ ] headless.json
- [ ] headless_backingstore.cpp
- [ ] headless_backingstore.h
- [ ] headless_integration.cpp
- [ ] headless_integration.h
- [ ] main.cpp

### library

- [ ] add_to_library.py
- [ ] caches.py
- [ ] check_library.py
- [ ] coloring.py
- [ ] comments.py
- [ ] custom_columns.py
- [ ] database.py
- [ ] database2.py
- [ ] field_metadata.py
- [ ] prefs.py
- [ ] restore.py
- [ ] save_to_disk.py
- [ ] schema_upgrades.py
- [ ] sqlite.py
- [ ] sqlite_custom.c
- [ ] __init__.py

#### catalogs

- [ ] bibtex.py
- [ ] csv_xml.py
- [ ] epub_mobi.py
- [ ] epub_mobi_builder.py
- [ ] utils.py
- [ ] __init__.py

### scraper

- [ ] qt.py
- [ ] qt_backend.py
- [ ] simple.py
- [ ] test_fetch_backend.py
- [ ] webengine_backend.py
- [ ] __init__.py

### spell

- [ ] break_iterator.py
- [ ] dictionary.py
- [ ] import_from.py
- [ ] __init__.py

### srv

- [ ] ajax.py
- [ ] auth.py
- [ ] auto_reload.py
- [ ] bonjour.py
- [ ] books.py
- [ ] cdb.py
- [ ] changes.py
- [ ] code.py
- [ ] content.py
- [ ] convert.py
- [ ] embedded.py
- [ ] errors.py
- [ ] fast_css_transform.cpp
- [ ] fts.py
- [ ] handler.py
- [ ] html_as_json.cpp
- [ ] http_request.py
- [ ] http_response.py
- [ ] jobs.py
- [ ] last_read.py
- [ ] legacy.py
- [ ] library_broker.py
- [ ] loop.py
- [ ] manage_users_cli.py
- [ ] metadata.py
- [ ] opds.py
- [ ] opts.py
- [ ] pool.py
- [ ] pre_activated.py
- [ ] render_book.py
- [ ] routes.py
- [ ] standalone.py
- [ ] TODO.rst
- [ ] users.py
- [ ] users_api.py
- [ ] utils.py
- [ ] web_socket.py
- [ ] __init__.py

### translations

- [ ] dynamic.py
- [ ] msgfmt.py
- [ ] __init__.py

### utils

- [ ] bibtex.py
- [ ] browser.py
- [ ] certgen.c
- [ ] certgen.py
- [ ] cleantext.py
- [ ] cocoa.m
- [ ] complete.py
- [x] config.py
- [x] config_base.py
- [ ] copy_files.py
- [ ] copy_files_test.py
- [ ] cpp_binding.h
- [x] date.py
- [ ] exim.py
- [ ] ffml_processor.py
- [ ] ffmpeg.c
- [x] filenames.py
- [ ] file_type_icons.py
- [ ] forked_map.py
- [ ] formatter.py
- [ ] formatter_functions.py
- [x] html2text.py
- [ ] https.py
- [ ] icu.c
- [ ] icu.py
- [ ] icu_calibre_utils.h
- [ ] icu_test.py
- [ ] img.py
- [x] imghdr.py
- [ ] inotify.py
- [ ] iphlpapi.py
- [ ] ipython.py
- [ ] ip_routing.py
- [x] iso8601.py
- [ ] linux_trash.py
- [ ] localization.py
- [ ] localunzip.py
- [ ] lock.py
- [x] logging.py
- [ ] matcher.c
- [ ] matcher.py
- [ ] mdns.py
- [x] network.py
- [x] ordered_dict.py
- [x] random_ua.py
- [ ] rapydscript.py
- [x] recycle_bin.py
- [x] resources.py
- [ ] run_tests.py
- [ ] safe_atexit.py
- [x] search_query_parser.py
- [x] search_query_parser_test.py
- [ ] serialize.py
- [x] seven_zip.py
- [ ] shared_file.py
- [ ] shm.py
- [x] short_uuid.py
- [x] smartypants.py
- [ ] smtp.py
- [ ] smtplib.py
- [x] socket_inheritance.py
- [ ] speedup.c
- [x] speedups.py
- [ ] tdir_in_cache.py
- [x] terminal.py
- [ ] test_lock.py
- [x] text2int.py
- [ ] threadpool.py
- [x] titlecase.py
- [ ] unicode-test.opf
- [x] unicode_names.py
- [x] unrar.py
- [x] unsmarten.py
- [ ] webengine.py
- [x] wordcount.py
- [x] xml_parse.py
- [ ] zipfile.py
- [ ] __init__.py

#### fonts

- [ ] freetype.cpp
- [ ] free_type.py
- [ ] metadata.py
- [ ] scanner.py
- [ ] subset.py
- [ ] utils.py
- [ ] winfonts.cpp
- [ ] win_fonts.py
- [ ] __init__.py

##### sfnt

- [ ] cmap.py
- [ ] common.py
- [ ] container.py
- [ ] errors.py
- [ ] glyf.py
- [ ] gsub.py
- [ ] head.py
- [ ] kern.py
- [ ] loca.py
- [ ] maxp.py
- [ ] merge.py
- [ ] metrics.py
- [ ] subset.py
- [ ] __init__.py

###### cff

- [ ] constants.py
- [ ] dict_data.py
- [ ] table.py
- [ ] writer.py
- [ ] __init__.py

#### hyphenation

- [ ] dictionaries.py
- [ ] hyphen.c
- [ ] hyphenate.py
- [ ] test_hyphenation.py
- [ ] __init__.py

#### imageops

- [ ] imageops.cpp
- [ ] imageops.h
- [ ] imageops.sip
- [ ] ordered_dither.cpp
- [ ] quantize.cpp

#### ipc

- [ ] job.py
- [ ] launch.py
- [ ] pool.py
- [ ] server.py
- [ ] simple_worker.py
- [ ] worker.py
- [ ] __init__.py

#### lzx

- [ ] compressor.c
- [ ] lzc.c
- [ ] lzc.h
- [ ] lzxc.c
- [ ] lzxc.h
- [ ] lzxd.c
- [ ] lzxd.h
- [ ] lzxmodule.c
- [ ] mspack.h
- [ ] system.h

#### magick

- [ ] draw.py
- [ ] legacy.py
- [ ] __init__.py

#### msdes

- [ ] d3des.h
- [ ] des.c
- [ ] msdesmodule.c
- [ ] spr.h

#### open_with

- [ ] linux.py
- [ ] osx.py
- [ ] windows.py
- [ ] __init__.py

#### opensearch

- [ ] description.py
- [ ] query.py
- [ ] url.py
- [ ] __init__.py

#### podofo

- [ ] doc.cpp
- [ ] fonts.cpp
- [ ] global.h
- [ ] images.cpp
- [ ] impose.cpp
- [ ] outline.cpp
- [ ] outlines.cpp
- [ ] output.cpp
- [ ] podofo.cpp
- [ ] test.cpp
- [ ] utils.cpp
- [ ] __init__.py

#### rcc

- [ ] rcc.cpp
- [ ] rcc.h
- [ ] rcc.sip
- [ ] __init__.py

#### spell

- [ ] hunspell_wrapper.cpp
- [ ] __init__.py

#### tts

- [ ] piper.cpp
- [ ] piper.py
- [ ] __init__.py

#### windows

- [ ] common.h
- [ ] wintest.py
- [ ] wintoast.cpp
- [ ] wintoastlib.cpp
- [ ] wintoastlib.h
- [ ] winutil.cpp
- [ ] __init__.py

#### winreg

- [ ] dde.py
- [ ] default_programs.py
- [ ] lib.py
- [ ] __init__.py

#### wmf

- [ ] emf.py
- [ ] parse.py
- [ ] __init__.py

### web

- [ ] __init__.py

#### feeds

- [ ] news.py
- [ ] templates.py
- [ ] __init__.py

##### recipes

- [ ] collection.py
- [ ] model.py
- [ ] __init__.py

#### fetch

- [ ] simple.py
- [ ] utils.py
- [ ] __init__.py

#### site_parsers

- [ ] natgeo.py
- [ ] nytimes.py
- [ ] __init__.py

## src/css_selectors

- [ ] errors.py
- [ ] ordered_set.py
- [ ] parser.py
- [ ] select.py
- [ ] tests.py
- [ ] __init__.py

## src/odf

- [ ] anim.py
- [ ] attrconverters.py
- [ ] chart.py
- [ ] config.py
- [ ] dc.py
- [ ] dr3d.py
- [ ] draw.py
- [ ] easyliststyle.py
- [ ] element.py
- [ ] elementtypes.py
- [ ] form.py
- [ ] grammar.py
- [ ] load.py
- [ ] manifest.py
- [ ] math.py
- [ ] meta.py
- [ ] namespaces.py
- [ ] number.py
- [ ] odf2moinmoin.py
- [ ] odf2xhtml.py
- [ ] odfmanifest.py
- [ ] office.py
- [ ] opendocument.py
- [ ] presentation.py
- [ ] script.py
- [ ] style.py
- [ ] svg.py
- [ ] table.py
- [ ] teletype.py
- [ ] text.py
- [ ] thumbnail.py
- [ ] userfield.py
- [ ] xforms.py
- [ ] __init__.py

## src/perfect-hashing

### frozen

- [ ] algorithm.h
- [ ] CMakeLists.txt
- [ ] map.h
- [ ] random.h
- [ ] set.h
- [ ] string.h
- [ ] unordered_map.h
- [ ] unordered_set.h

#### bits

- [ ] algorithms.h
- [ ] basic_types.h
- [ ] constexpr_assert.h
- [ ] defines.h
- [ ] elsa.h
- [ ] exceptions.h
- [ ] pmh.h
- [ ] version.h

## src/polyglot

- [ ] binary.py
- [ ] builtins.py
- [ ] functools.py
- [ ] html_entities.py
- [ ] http_client.py
- [ ] http_cookie.py
- [ ] http_server.py
- [ ] io.py
- [ ] plistlib.py
- [ ] queue.py
- [ ] reprlib.py
- [ ] smtplib.py
- [ ] socketserver.py
- [ ] urllib.py
- [ ] __init__.py

## src/pyj

- [ ] ajax.pyj
- [ ] autoreload.pyj
- [ ] complete.pyj
- [ ] date.pyj
- [ ] dom.pyj
- [ ] editor.pyj
- [ ] file_uploads.pyj
- [ ] fs_images.pyj
- [ ] iframe_comm.pyj
- [ ] image_popup.pyj
- [ ] initialize.pyj
- [ ] live_css.pyj
- [ ] lru_cache.pyj
- [ ] modals.pyj
- [ ] popups.pyj
- [ ] qt.pyj
- [ ] range_utils.pyj
- [ ] select.pyj
- [ ] session.pyj
- [ ] srv.pyj
- [ ] test.pyj
- [ ] testing.pyj
- [ ] test_annotations.pyj
- [ ] test_date.pyj
- [ ] test_utils.pyj
- [ ] utils.pyj
- [ ] viewer-main.pyj
- [ ] widgets.pyj
- [ ] worker.pyj

### book_list

- [ ] add.pyj
- [ ] book_details.pyj
- [ ] comments_editor.pyj
- [ ] constants.pyj
- [ ] conversion_widgets.pyj
- [ ] convert_book.pyj
- [ ] cover_grid.pyj
- [ ] custom_list.pyj
- [ ] delete_book.pyj
- [ ] details_list.pyj
- [ ] edit_metadata.pyj
- [ ] fts.pyj
- [ ] globals.pyj
- [ ] home.pyj
- [ ] item_list.pyj
- [ ] library_data.pyj
- [ ] local_books.pyj
- [ ] main.pyj
- [ ] prefs.pyj
- [ ] router.pyj
- [ ] search.pyj
- [ ] show_note.pyj
- [ ] theme.pyj
- [ ] top_bar.pyj
- [ ] ui.pyj
- [ ] views.pyj
- [ ] __init__.pyj

### read_book

- [ ] anchor_visibility.pyj
- [ ] annotations.pyj
- [ ] bookmarks.pyj
- [ ] cfi.pyj
- [ ] content_popup.pyj
- [ ] db.pyj
- [ ] extract.pyj
- [ ] find.pyj
- [ ] flow_mode.pyj
- [ ] footnotes.pyj
- [ ] gestures.pyj
- [ ] globals.pyj
- [ ] goto.pyj
- [ ] highlights.pyj
- [ ] hints.pyj
- [ ] iframe.pyj
- [ ] mathjax.pyj
- [ ] open_book.pyj
- [ ] overlay.pyj
- [ ] paged_mode.pyj
- [ ] profiles.pyj
- [ ] read_aloud.pyj
- [ ] read_audio_ebook.pyj
- [ ] referencing.pyj
- [ ] resources.pyj
- [ ] scrollbar.pyj
- [ ] search.pyj
- [ ] search_worker.pyj
- [ ] selection_bar.pyj
- [ ] settings.pyj
- [ ] shortcuts.pyj
- [ ] smil.pyj
- [ ] test_cfi.pyj
- [ ] timers.pyj
- [ ] toc.pyj
- [ ] touch.pyj
- [ ] tts.pyj
- [ ] ui.pyj
- [ ] view.pyj
- [ ] viewport.pyj
- [ ] word_actions.pyj
- [ ] __init__.pyj

#### prefs

- [ ] colors.pyj
- [ ] fonts.pyj
- [ ] font_size.pyj
- [ ] head_foot.pyj
- [ ] keyboard.pyj
- [ ] layout.pyj
- [ ] main.pyj
- [ ] misc.pyj
- [ ] scrolling.pyj
- [ ] selection.pyj
- [ ] touch.pyj
- [ ] user_stylesheet.pyj
- [ ] utils.pyj
- [ ] __init__.pyj

### viewer

- [ ] constants.pyj
- [ ] router.pyj
- [ ] __init__.pyj

## src/qt

- [ ] core.py
- [ ] core.pyi
- [ ] core_name_map.py
- [ ] dbus.py
- [ ] dbus.pyi
- [ ] dbus_name_map.py
- [ ] loader.py
- [ ] webengine.py
- [ ] webengine.pyi
- [ ] webengine_name_map.py
- [ ] __init__.py
- [ ] __main__.py

## src/templite

- [ ] __init__.py

## src/tinycss

- [ ] color3.py
- [ ] css21.py
- [ ] decoding.py
- [ ] fonts3.py
- [ ] media3.py
- [ ] page3.py
- [ ] parsing.py
- [ ] tokenizer.c
- [ ] tokenizer.py
- [ ] token_data.py
- [ ] version.py
- [ ] __init__.py

## src/unicode_names

- [ ] data-types.h
- [ ] names.h
- [ ] unicode_names.c

