// What the library view knows about the library it is showing, and the banners
// and prompts that hang off it: its name and where it lives, the storage and
// change-log advisories, "this folder has files that are not in the library", a
// job the previous server process left unfinished, and creating or switching
// library (the desktop app).
//
// Switch-library quick-switch (issue #725, desktop-only). This app stays
// single-library-per-instance -- "switching" re-spawns lexmechanic_srv against a
// different path and re-navigates the window (app/src-tauri/src/lib.rs's
// open_library/open_recent_library), it doesn't serve multiple libraries at once.
// The list is capped/deduplicated server-side (app/src-tauri/src/settings.rs).

import { computed, ref } from "vue";

import { confirmDialog } from "../components/dialogs/confirm";
import { tauriInvoke } from "../tauri";
import {
  cancelLibraryJob, describeJobProgress, fetchChangeLogStatus, fetchLibraryInfo, fetchLibraryJobs, fetchUntracked, repairChangeLog, resumableJob, resumeLibraryJob, runningLibraryJob, scanLibrary, switchWarning,
  type LibraryJob,
} from "./api";
import { changeLogBanner, type ChangeLogStatus } from "./changeLog";
import { describeScanOutcome, dismissKey, type UntrackedCount } from "./scanPrompt";
import { storageBanner, type StorageInfo } from "./storageBanner";

export function useLibraryInfo({ libraryChanged }: { libraryChanged: () => Promise<void> }) {
  const libraryName = ref("");
  const libraryPath = ref("");
  const storage = ref<StorageInfo | null>(null);
  const banner = computed(() => storageBanner(storage.value));
  const bannerDismissed = ref(false);

  // Every library this server hosts, for the copy-to-library target picker
  // (#959). One entry when the server was started with a single library,
  // which is why the action says so rather than opening an empty list.
  const libraryId = ref("");
  const libraryMap = ref<Record<string, string>>({});

  const copyTargets = computed(() => Object.entries(libraryMap.value).filter(([id]) => id !== libraryId.value).map(([id, name]) => ({ id, name })));

  // The change log fell behind the database (an append failed): say so, and
  // offer the repair that makes the log whole again. Checked when the library
  // opens and after anything that changes it (see `libraryChanged`).
  const changeLogStatus = ref<ChangeLogStatus | null>(null);
  const changeLogWarning = computed(() => changeLogBanner(changeLogStatus.value));
  const changeLogRepairBusy = ref(false);
  const changeLogRepairError = ref<string | null>(null);

  async function refreshChangeLogStatus() {
    try {
      changeLogStatus.value = await fetchChangeLogStatus();
    } catch {
      // An older server has no such route, or it is unreachable: no warning.
    }
  }

  async function repairChangeLogNow() {
    changeLogRepairBusy.value = true;
    changeLogRepairError.value = null;
    try {
      changeLogStatus.value = await repairChangeLog();
    } catch (e) {
      changeLogRepairError.value = e instanceof Error ? e.message : String(e);
    } finally {
      changeLogRepairBusy.value = false;
    }
  }

  async function loadLibraryInfo() {
    void refreshChangeLogStatus();
    try {
      const info = await fetchLibraryInfo();
      storage.value = info.storage;
      libraryName.value = info.name;
      libraryPath.value = info.path;
      libraryId.value = info.id;
      libraryMap.value = info.libraries;
      void checkUntracked();
    } catch {
      // Not worth surfacing: the status bar simply omits the label, and
      // every other part of the window works without it.
    }
  }

  // The library, as a folder somebody filled: opening it does not scan it, and on
  // a share a scan is long, so ask first. `untracked` is a walk with no reads.
  const untracked = ref<UntrackedCount | null>(null);
  const scanPromptDismissed = ref(false);
  const promptScanBusy = ref(false);
  const promptScanProgress = ref<string | null>(null);
  const promptScanJobId = ref<number | null>(null);
  const promptScanSummary = ref<string | null>(null);
  const promptScanError = ref<string | null>(null);

  function readDismissed(path: string): boolean {
    try {
      return sessionStorage.getItem(dismissKey(path)) === "1";
    } catch {
      return false;
    }
  }

  async function checkUntracked() {
    try {
      const found = await fetchUntracked();
      scanPromptDismissed.value = readDismissed(libraryPath.value);
      untracked.value = found;
    } catch {
      // A prompt is a courtesy; the library works without it.
    }
  }

  function dismissScanPrompt() {
    scanPromptDismissed.value = true;
    try {
      sessionStorage.setItem(dismissKey(libraryPath.value), "1");
    } catch {
      // Remembered for this page only.
    }
  }

  function closeScanResult() {
    promptScanSummary.value = null;
    promptScanError.value = null;
    dismissScanPrompt();
  }

  async function scanFromPrompt() {
    promptScanBusy.value = true;
    promptScanError.value = null;
    promptScanSummary.value = null;
    try {
      const result = await scanLibrary({
        onStart: (id) => (promptScanJobId.value = id),
        onProgress: (job) => (promptScanProgress.value = describeJobProgress(job.progress)),
      });
      promptScanSummary.value = describeScanOutcome(result);
      await libraryChanged();
      await checkUntracked();
    } catch (e) {
      promptScanError.value = e instanceof Error ? e.message : String(e);
    } finally {
      promptScanBusy.value = false;
      promptScanProgress.value = null;
      promptScanJobId.value = null;
    }
  }

  async function stopPromptScan() {
    if (promptScanJobId.value === null) return;
    try {
      await cancelLibraryJob(promptScanJobId.value);
    } catch (e) {
      promptScanError.value = e instanceof Error ? e.message : String(e);
    }
  }

  // A job the previous server process was running when it ended (a crash, or
  // switching library) comes back from `.lexmechanic/jobs.json` as
  // `interrupted`; the resumable kinds offer to continue.
  const interruptedJob = ref<LibraryJob | null>(null);
  const resumeBusy = ref(false);
  const resumeProgress = ref<string | null>(null);
  const resumeError = ref<string | null>(null);

  async function loadInterruptedJob() {
    try {
      interruptedJob.value = resumableJob(await fetchLibraryJobs());
    } catch {
      // An older server has no such route; there is nothing to offer.
    }
  }

  async function resumeInterrupted() {
    const job = interruptedJob.value;
    if (!job) return;
    resumeBusy.value = true;
    resumeError.value = null;
    try {
      await resumeLibraryJob(job, { onProgress: (j) => (resumeProgress.value = describeJobProgress(j.progress)) });
      interruptedJob.value = null;
      await loadLibraryInfo();
    } catch (e) {
      resumeError.value = e instanceof Error ? e.message : String(e);
    } finally {
      resumeBusy.value = false;
      resumeProgress.value = null;
    }
  }

  const newLibraryOpen = ref(false);
  const newLibraryName = ref("");
  const newLibraryError = ref<string | null>(null);
  const creatingLibrary = ref(false);

  function openNewLibrary() {
    newLibraryName.value = "";
    newLibraryError.value = null;
    newLibraryOpen.value = true;
  }

  /**
   * Creates a library and opens it.
   *
   * The name is collected here and the *parent* folder in the native
   * dialog that follows, rather than asking for one folder: "where shall
   * I put it" and "what is it called" are different questions, and
   * merging them is how a library ends up created at the root of
   * someone's documents folder.
   */
  async function createLibrary() {
    const name = newLibraryName.value.trim();
    if (!name) {
      newLibraryError.value = "Give the library a name.";
      return;
    }
    if (!(await confirmSwitchOverRunningJob())) return;
    creatingLibrary.value = true;
    newLibraryError.value = null;
    try {
      const created = await tauriInvoke<string | null>("create_library", { name });
      // `null` means the folder dialog was cancelled, which is not an
      // error -- leave the prompt open so the name typed is not lost.
      if (created) newLibraryOpen.value = false;
    } catch (e) {
      newLibraryError.value = e instanceof Error ? e.message : String(e);
    } finally {
      creatingLibrary.value = false;
    }
  }

  const switchOpen = ref(false);
  const recentLibraries = ref<string[]>([]);
  const switching = ref(false);
  const switchError = ref<string | null>(null);

  /**
   * Refreshes the recents list.
   *
   * Failure is swallowed: recents are a convenience, and the dialog and
   * menu both still offer the two real routes without them. A browser
   * tab has no such command at all, which is the common case rather than
   * an error.
   */
  async function loadRecentLibraries() {
    try {
      recentLibraries.value = await tauriInvoke<string[]>("list_recent_libraries");
    } catch {
      recentLibraries.value = [];
    }
  }

  async function openSwitchLibrary() {
    switchError.value = null;
    switchOpen.value = true;
    await loadRecentLibraries();
  }

  /**
   * Replacing the library's server kills whatever job it is running, so ask
   * first. A failed check (an older server, a network blip) does not block
   * the switch: the user asked for it.
   */
  async function confirmSwitchOverRunningJob(): Promise<boolean> {
    try {
      const warning = switchWarning(await runningLibraryJob());
      return warning === null || (await confirmDialog(warning, { title: "Switch library", confirmLabel: "Switch" }));
    } catch {
      return true;
    }
  }

  async function switchToRecent(path: string) {
    if (!(await confirmSwitchOverRunningJob())) return;
    switching.value = true;
    switchError.value = null;
    try {
      await tauriInvoke<void>("open_recent_library", { path });
      switchOpen.value = false;
    } catch (e) {
      switchError.value = e instanceof Error ? e.message : String(e);
    } finally {
      switching.value = false;
    }
  }

  async function switchToOther() {
    if (!(await confirmSwitchOverRunningJob())) return;
    switching.value = true;
    switchError.value = null;
    try {
      const picked = await tauriInvoke<boolean>("choose_library");
      if (picked) switchOpen.value = false;
    } catch (e) {
      switchError.value = e instanceof Error ? e.message : String(e);
    } finally {
      switching.value = false;
    }
  }

  return {
    libraryName, libraryPath, libraryId, libraryMap, copyTargets, storage, banner, bannerDismissed,
    changeLogWarning, changeLogRepairBusy, changeLogRepairError, refreshChangeLogStatus, repairChangeLogNow,
    loadLibraryInfo,
    untracked, scanPromptDismissed, promptScanBusy, promptScanProgress, promptScanSummary, promptScanError, scanFromPrompt, stopPromptScan, dismissScanPrompt, closeScanResult,
    interruptedJob, resumeBusy, resumeProgress, resumeError, loadInterruptedJob, resumeInterrupted,
    newLibraryOpen, newLibraryName, newLibraryError, creatingLibrary, openNewLibrary, createLibrary,
    switchOpen, recentLibraries, switching, switchError, loadRecentLibraries, openSwitchLibrary, switchToRecent, switchToOther,
  };
}
