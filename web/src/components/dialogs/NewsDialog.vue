<script setup lang="ts">
// Fetch news/recipes -- a real, generic RSS/Atom feed reader (see
// crates/lexmechanic_srv/src/news.rs's own doc for why this isn't a catalog of
// upstream's ~1077 hand-written per-site recipes). It stays mounted while
// closed, so the feeds typed and a fetch still running survive closing it.

import { ref, useId, watch } from "vue";

import { addNewsSchedule, getNewsFetchStatus, importOpml, listNewsSchedules, removeNewsSchedule, runNewsScheduleNow, startNewsFetch } from "../../library/api";
import type { CustomRecipeOptions, NewsFeedInput, NewsSchedule } from "../../library/api";
import Dialog from "../Dialog.vue";

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{
  close: [];
  /** A news book was added: the list is stale. */
  changed: [];
}>();
const formId = `news-form-${useId()}`;

const newsTitle = ref("");
const newsFeedUrls = ref("");
const newsOldestArticleDays = ref("");
const newsMaxArticlesPerFeed = ref("");
const newsFetching = ref(false);
const newsError = ref<string | null>(null);
const newsDone = ref(false);

// Scheduled feeds (#764) -- a saved feed configuration that runs
// automatically on a real recurring interval via a server-side
// background task, instead of only this panel's own one-shot fetch.
const newsSchedules = ref<NewsSchedule[]>([]);
const scheduleIntervalMinutes = ref("1440");
const schedulingBusy = ref(false);
const scheduleError = ref<string | null>(null);

async function loadNewsSchedules() {
  try {
    const { schedules } = await listNewsSchedules();
    newsSchedules.value = schedules;
  } catch (e) {
    scheduleError.value = e instanceof Error ? e.message : String(e);
  }
}

// Opening starts clean on what happened last time, but keeps what was typed.
watch(
  () => props.open,
  (open) => {
    if (!open) return;
    newsError.value = null;
    newsDone.value = false;
    void loadNewsSchedules();
  },
);

async function saveNewsSchedule() {
  // Real, disclosed narrowing: scheduled feeds (#764) only store plain
  // URLs, not #765's {title, url} sections -- strip any "Title|" prefix
  // rather than saving a broken literal URL.
  const feeds = parseFeedLines(newsFeedUrls.value).map((f) => (typeof f === "string" ? f : f.url));
  const minutes = Number(scheduleIntervalMinutes.value);
  if (feeds.length === 0 || !Number.isFinite(minutes) || minutes < 1) return;
  schedulingBusy.value = true;
  scheduleError.value = null;
  try {
    await addNewsSchedule(newsTitle.value.trim(), feeds, Math.round(minutes * 60));
    await loadNewsSchedules();
  } catch (e) {
    scheduleError.value = e instanceof Error ? e.message : String(e);
  } finally {
    schedulingBusy.value = false;
  }
}

async function deleteNewsSchedule(id: number) {
  await removeNewsSchedule(id);
  await loadNewsSchedules();
}

async function runNewsScheduleNowClick(id: number) {
  schedulingBusy.value = true;
  scheduleError.value = null;
  try {
    const result = await runNewsScheduleNow(id);
    if (!result.ok) throw new Error(result.error || "run failed");
    emit("changed");
    await loadNewsSchedules();
  } catch (e) {
    scheduleError.value = e instanceof Error ? e.message : String(e);
  } finally {
    schedulingBusy.value = false;
  }
}

const opmlInput = ref<HTMLInputElement | null>(null);
const opmlImporting = ref(false);
const opmlSummary = ref<string | null>(null);

async function importOpmlFile(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;
  opmlImporting.value = true;
  opmlSummary.value = null;
  newsError.value = null;
  try {
    const text = await file.text();
    const feeds = await importOpml(text);
    if (feeds.length === 0) {
      opmlSummary.value = "No feeds found in that OPML file.";
      return;
    }
    const existing = newsFeedUrls.value.split("\n").map((u) => u.trim()).filter(Boolean);
    const newUrls = feeds.map((f) => f.feed_url).filter((u) => !existing.includes(u));
    newsFeedUrls.value = [...existing, ...newUrls].join("\n");
    opmlSummary.value = `Added ${newUrls.length} feed(s) from OPML${newUrls.length < feeds.length ? ` (${feeds.length - newUrls.length} already listed)` : ""}.`;
  } catch (e) {
    newsError.value = e instanceof Error ? e.message : String(e);
  } finally {
    opmlImporting.value = false;
    if (opmlInput.value) opmlInput.value.value = "";
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// A line may be a bare URL, or "Section Title|https://..." naming its
// own real recipe section (#765) -- reuses the existing one-line-per-
// feed textarea rather than a new per-row form for this first slice.
function parseFeedLines(raw: string): NewsFeedInput[] {
  return raw
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const pipe = line.indexOf("|");
      if (pipe === -1) return line;
      const title = line.slice(0, pipe).trim();
      const url = line.slice(pipe + 1).trim();
      return title && url ? { title, url } : line;
    });
}

async function fetchNews() {
  const feeds = parseFeedLines(newsFeedUrls.value);
  if (feeds.length === 0) return;
  newsFetching.value = true;
  newsError.value = null;
  newsDone.value = false;
  try {
    const options: CustomRecipeOptions = {};
    if (newsOldestArticleDays.value.trim()) options.oldestArticleDays = Number(newsOldestArticleDays.value);
    if (newsMaxArticlesPerFeed.value.trim()) options.maxArticlesPerFeed = Number(newsMaxArticlesPerFeed.value);
    const jobId = await startNewsFetch(newsTitle.value.trim(), feeds, options);
    for (;;) {
      const status = await getNewsFetchStatus(jobId);
      if (!status.running) {
        if (!status.ok) throw new Error(status.error || "fetch failed");
        break;
      }
      await sleep(1000);
    }
    newsDone.value = true;
    emit("changed");
  } catch (e) {
    newsError.value = e instanceof Error ? e.message : String(e);
  } finally {
    newsFetching.value = false;
  }
}
</script>

<template>
    <Dialog :open="open" title="Fetch news" show-close size="md" @close="emit('close')">
      <p class="news-hint">Enter one or more RSS/Atom feed URLs, one per line. Prefix a line with "Section Title|" to name that feed's own section in a multi-section recipe. This downloads the latest articles and adds the result as a new book.</p>
      <form :id="formId" @submit.prevent="fetchNews">
        <label class="news-field">
          Title
          <input v-model="newsTitle" placeholder="e.g. My Weekly" :disabled="newsFetching" />
        </label>
        <label class="news-field">
          Feed URLs
          <textarea v-model="newsFeedUrls" rows="4" placeholder="e.g. https://example.com/feed.xml&#10;Tech News|https://example.com/tech.xml" :disabled="newsFetching"></textarea>
        </label>
        <div class="bulk-actions">
          <button type="button" :disabled="newsFetching || opmlImporting" @click="opmlInput?.click()">{{ opmlImporting ? "Importing…" : "Import OPML…" }}</button>
          <input ref="opmlInput" type="file" accept=".opml,.xml,text/x-opml,text/xml" class="hidden-file-input" @change="importOpmlFile" />
        </div>
        <label class="news-field">
          Oldest article (days)
          <input v-model="newsOldestArticleDays" type="number" min="0" step="1" placeholder="7" :disabled="newsFetching" />
        </label>
        <label class="news-field">
          Max articles per feed
          <input v-model="newsMaxArticlesPerFeed" type="number" min="1" step="1" placeholder="100" :disabled="newsFetching" />
        </label>
        <label class="news-field">
          Repeat every (minutes)
          <input v-model="scheduleIntervalMinutes" type="number" min="1" step="1" :disabled="schedulingBusy" />
        </label>
      </form>
      <p v-if="opmlSummary" class="news-hint">{{ opmlSummary }}</p>
      <p v-if="newsDone" class="news-done">Added the fetched news as a new book.</p>
      <p v-if="newsError" class="error">{{ newsError }}</p>
      <p v-if="scheduleError" class="error">{{ scheduleError }}</p>

      <template v-if="newsSchedules.length">
        <h4>Scheduled feeds</h4>
        <ul class="manage-list">
          <li v-for="s in newsSchedules" :key="s.id">
            <span class="manage-name">{{ s.title }} — every {{ Math.round(s.interval_secs / 60) }} min</span>
            <code class="manage-query">next: {{ new Date(s.next_run_at).toLocaleString() }}<template v-if="s.last_result"> · last: {{ s.last_result }}</template></code>
            <button type="button" :disabled="schedulingBusy" @click="runNewsScheduleNowClick(s.id)">Run now</button>
            <button type="button" :disabled="schedulingBusy" @click="deleteNewsSchedule(s.id)">Delete</button>
          </li>
        </ul>
      </template>
      <template #footer>
        <button type="button" :disabled="schedulingBusy || !newsFeedUrls.trim()" @click="saveNewsSchedule">{{ schedulingBusy ? "Saving…" : "Save schedule…" }}</button>
        <button type="button" :disabled="newsFetching" @click="emit('close')">Close</button>
        <button type="submit" :form="formId" class="primary" :disabled="newsFetching || !newsFeedUrls.trim()">{{ newsFetching ? "Fetching…" : "Fetch once" }}</button>
      </template>
    </Dialog>
</template>

<style scoped>
.bulk-actions {
  margin-top: 0.6em;
  display: flex;
  gap: 0.5em;
}
.manage-name {
  font-weight: 600;
  flex-shrink: 0;
}
.manage-query {
  color: var(--fg-muted);
  font-size: var(--fs-small);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}
.error {
  padding: 1em;
  color: var(--danger);
}
.news-field {
  display: flex;
  flex-direction: column;
  gap: 0.3em;
  font-size: var(--fs-small);
  color: var(--fg-muted);
  margin-bottom: 0.75em;
}
.news-field input,
.news-field textarea {
  font: inherit;
  padding: 0.35em 0.5em;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  resize: vertical;
}
.news-done {
  color: var(--success);
  margin: 0.6em 0 0;
}
.hidden-file-input {
  display: none;
}
</style>
