import { licenseText } from './license.js';
import { fetchLanguage, invalidJSON } from './language.js';
import { renderMarkdown, pickAssetForPlatform } from './update.js';
import {
  initSettingsStore,
  setDynamicDefault,
  refreshSettingsState,
  localizeSettingsStore,
  cancelSettingsResetConfirm,
  flushSettingsStore,
  exportSettings,
  importSettings,
} from './settings-store.js';
import { initSettingsLayout, syncSettingsLayout } from './settings-layout.js';

let invoke;
if (window.__TAURI__) {
  invoke = window.__TAURI__.core.invoke;
} else {
  function dummyFunc() { }
  window.__TAURI__ = { event: { listen: dummyFunc } };
  invoke = dummyFunc;
}

const DEFAULT_LOCALE_PATH = `./locales/en.json`;

// Track current bbox selection info localization key for language changes
let currentBboxSelectionKey = "select_area_prompt";
let currentBboxSelectionColor = "#ffffff";
// Values for the {placeholders} of the current key, or null.
let currentBboxSelectionVars = null;

function fillBboxSelectionVars(element) {
  for (const k in (currentBboxSelectionVars || {})) {
    element.textContent = element.textContent.split('{' + k + '}').join(currentBboxSelectionVars[k]);
  }
}

// Helper function to set bbox selection info text and track it for language changes
async function setBboxSelectionInfo(bboxSelectionElement, localizationKey, color, vars) {
  currentBboxSelectionKey = localizationKey;
  currentBboxSelectionColor = color;
  currentBboxSelectionVars = vars || null;
  
  // Ensure localization is available
  let localization = window.localization;
  if (!localization) {
    localization = await getLocalization();
  }
  
  await localizeElement(localization, { element: bboxSelectionElement }, localizationKey);
  fillBboxSelectionVars(bboxSelectionElement);
  bboxSelectionElement.style.color = color;
}

// Initialize elements and start the demo progress
window.addEventListener("DOMContentLoaded", async () => {
  registerMessageEvent();
  window.startGeneration = startGeneration;
  setupProgressListener();
  await initSavePath();
  initSettings();
  // Before the store restores values, so the cards follow the restored ones.
  initSettingsLayout();
  initDialogs();
  initVoxyLightingCoupling();
  initCavesFillCoupling();
  refreshHeightLimitRow();
  initAdvancedFeatures();
  // After initSettings(), so the slider label and rotation handlers exist
  // before restored values are applied. Labels get localized a few lines below.
  initSettingsStore({ resetWorldFormat: () => setWorldFormat('java') });
  // The store's restore fired the toggle's change event before the save path
  // and the world name were known; read the world once more with both in hand.
  refreshOneWorldState();
  resolveDefaultSavePath();
  initTelemetryConsent();
  initClearCacheButton();
  initPrecomputeFacadesButton();
  initTooltips();
  handleBboxInput();
  const localization = await getLocalization();
  await applyLocalization(localization);
  updateFormatToggleUI(selectedWorldFormat);
  initFooter();
  initEasterEggs();
  checkForUpdates();
});

// Expose language functions to window for use by language-selector.js
window.fetchLanguage = fetchLanguage;
window.applyLocalization = applyLocalization;
window.initFooter = initFooter;

/**
 * Fetches and returns localization data based on user's language
 * Falls back to English if requested language is not available
 * @returns {Promise<Object>} The localization JSON object
 */
async function getLocalization() {
  // Check if user has a saved language preference
  const savedLanguage = localStorage.getItem('arnis-language');

  // If there's a saved preference, use it
  if (savedLanguage) {
    return await fetchLanguage(savedLanguage);
  }

  // Otherwise use the browser's language
  const lang = navigator.language;
  return await fetchLanguage(lang);
}

/**
 * Updates an HTML element with localized text
 * @param {Object} json - Localization data
 * @param {Object} elementObject - Object containing element or selector
 * @param {string} localizedStringKey - Key for the localized string
 */
async function localizeElement(json, elementObject, localizedStringKey) {
  const element =
    (!elementObject.element || elementObject.element === "")
      ? document.querySelector(elementObject.selector) : elementObject.element;
  const attribute = localizedStringKey.startsWith("placeholder_") ? "placeholder" : "textContent";

  if (element) {
    if (json && localizedStringKey in json) {
      element[attribute] = json[localizedStringKey];
    } else {
      // Fallback to default (English) string
      const defaultJson = await fetchLanguage('en');
      element[attribute] = defaultJson[localizedStringKey];
    }
  }
}

async function applyLocalization(localization) {
  const localizationElements = {
    "#start-button > span[data-localize='start_generation']": "start_generation",
    "#world-name-label[data-placeholder]": "no_world_generated_yet",
    "input[id='world-name-input']": "placeholder_world_name",
    // DEPRECATED: Ground level localization removed
    // "label[data-localize='ground_level']": "ground_level",
    ".footer-link": "footer_text",

    // Placeholder strings
    "input[id='bbox-coords']": "placeholder_bbox",
    "#bbox-features-cta": "area_use_extra_features",
    // DEPRECATED: Ground level placeholder removed
    // "input[id='ground-level']": "placeholder_ground"
  };

  for (const selector in localizationElements) {
    localizeElement(localization, { selector: selector }, localizationElements[selector]);
  }

  // Every text on the settings page and in the dialogs carries its key, and
  // several appear more than once (a section name is in the sidebar and on the
  // section itself), so they are localized in one pass rather than selector by
  // selector.
  document.querySelectorAll("#settings-modal [data-localize], .dialog [data-localize], #offline-panel [data-localize]").forEach((element) => {
    localizeElement(localization, { element }, element.dataset.localize);
  });

  // settings-store.js creates these buttons and owns their text.
  localizeSettingsStore(localization);

  // Re-apply current bbox selection info text with new language
  const bboxSelectionInfo = document.getElementById("bbox-selection-info");
  if (bboxSelectionInfo && currentBboxSelectionKey) {
    await localizeElement(localization, { element: bboxSelectionInfo }, currentBboxSelectionKey);
    fillBboxSelectionVars(bboxSelectionInfo);
    bboxSelectionInfo.style.color = currentBboxSelectionColor;
  }

  // Update error messages
  window.localization = localization;
  renderOneWorldStatus();
  formatCpuUsage();
  renderDataPlan();
  syncOfflineFirst();
  refreshOptionPreviews();
  // The map hint lives in the map iframe, which cannot see this assignment.
  document.querySelectorAll('iframe').forEach((frame) => {
    try {
      const w = frame.contentWindow;
      if (w && typeof w.renderBboxHint === 'function') w.renderBboxHint();
      // The toolbar's labels and the search placeholder, same reason.
      if (w && typeof w.refreshMapToolLabels === 'function') w.refreshMapToolLabels();
    } catch (_) {
      // A frame that is not ours or not loaded yet; the hint renders itself
      // once it is.
    }
  });

  // The line above has just written the idle label over a button that may be
  // saying Cancel, so put the running state back.
  refreshPrecomputeButton();
}

// Function to initialize the footer with the current year and version
async function initFooter() {
  const currentYear = new Date().getFullYear();
  let version = "x.x.x";

  try {
    version = await invoke('gui_get_version');
  } catch (error) {
    console.error("Failed to fetch version:", error);
  }

  const footerElement = document.querySelector(".footer-link");
  if (footerElement) {
    // Get the original text from localization if available, or use the current text
    let footerText = footerElement.textContent;

    // Check if the text is from localization and contains placeholders
    if (window.localization && window.localization.footer_text) {
      footerText = window.localization.footer_text;
    }

    // Replace placeholders with actual values
    footerElement.textContent = footerText
      .replace("{year}", currentYear)
      .replace("{version}", version);

    // The About section of the settings page repeats the same line.
    const aboutVersion = document.getElementById("about-version");
    if (aboutVersion) aboutVersion.textContent = footerElement.textContent;
  }
}

let latestReleaseInfo = null;
let currentPlatform = "unknown";

const SEEN_VERSION_KEY = "arnis-update-seen-version";

// Only forward http(s)/mailto URLs to the OS handler; reject javascript:/data:/file:/etc.
function isSafeExternalUrl(url) {
  return typeof url === "string" && /^(?:https?|mailto):/i.test(url);
}

async function openExternal(url) {
  if (!isSafeExternalUrl(url)) {
    console.warn("Refusing to open URL with disallowed scheme:", url);
    return;
  }
  try {
    if (window.__TAURI__ && window.__TAURI__.shell && window.__TAURI__.shell.open) {
      await window.__TAURI__.shell.open(url);
    } else {
      window.open(url, "_blank", "noopener,noreferrer");
    }
  } catch (err) {
    console.error("Failed to open URL:", url, err);
  }
}

async function checkForUpdates() {
  try {
    const [info, platform] = await Promise.all([
      invoke("gui_get_update_info"),
      invoke("gui_get_platform"),
    ]);
    latestReleaseInfo = info;
    currentPlatform = platform || "unknown";
    if (!info || !info.isNewer) return;

    const footer = document.querySelector(".footer");
    const updateMessage = document.createElement("span");
    updateMessage.setAttribute("role", "button");
    updateMessage.setAttribute("tabindex", "0");
    updateMessage.style.color = "#fecc44";
    updateMessage.style.marginTop = "-5px";
    updateMessage.style.fontSize = "0.95em";
    updateMessage.style.display = "block";
    updateMessage.style.cursor = "pointer";
    updateMessage.addEventListener("click", () => openUpdateModal());
    updateMessage.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openUpdateModal();
      }
    });
    localizeElement(window.localization, { element: updateMessage }, "new_version_available");
    footer.style.marginTop = "10px";
    footer.appendChild(updateMessage);

    // Auto-open once per new remote version; "seen" key records the last auto-shown version.
    const seen = localStorage.getItem(SEEN_VERSION_KEY);
    if (seen !== info.remoteVersion) {
      openUpdateModal();
    }
  } catch (error) {
    console.error("Failed to check for updates: ", error);
  }
}

function openUpdateModal(opts = {}) {
  const info = latestReleaseInfo;
  // Version info opened from Settings only reads, unless the release it shows
  // is newer than this build: then the download is offered there as well.
  const showDownload =
    opts.showDownload !== false || !!(info && info.release && info.isNewer);
  const modal = document.getElementById("update-modal");
  if (!modal) return;
  const titleEl = document.getElementById("update-modal-title");
  const bodyEl = document.getElementById("update-modal-body");
  const downloadBtn = document.getElementById("update-download-button");
  const downloadNote = document.getElementById("update-modal-download-note");

  // Hide download button + the "opens in your browser" note in version-info mode.
  if (downloadBtn) downloadBtn.style.display = showDownload ? "" : "none";
  if (downloadNote) downloadNote.style.display = showDownload ? "" : "none";

  const metaEl = document.getElementById("update-modal-meta");

  if (!info || !info.release) {
    titleEl.textContent =
      (window.localization && window.localization.update_modal_title) || "What's new in Arnis";
    if (metaEl) metaEl.replaceChildren();
    const fallbackMsg =
      (window.localization && window.localization.update_fetch_failed) ||
      "Could not fetch the latest release information. Please check your internet connection or visit GitHub directly.";
    bodyEl.innerHTML = `<p>${escapeHTMLLite(fallbackMsg)}</p>`;
    if (downloadBtn) downloadBtn.disabled = true;
    showModal(modal);
    return;
  }
  const rel = info.release;

  titleEl.textContent = (rel.name && rel.name.trim()) || rel.tag_name || "Latest release";
  renderReleaseMeta(metaEl, info, rel);
  bodyEl.innerHTML = renderMarkdown(rel.body || "");
  bodyEl.scrollTop = 0;
  bodyEl.querySelectorAll("a[href]").forEach((a) => {
    const href = a.getAttribute("href");
    if (!href) return;
    a.addEventListener("click", (e) => {
      e.preventDefault();
      openExternal(href);
    });
  });

  if (downloadBtn && showDownload) {
    const asset = pickAssetForPlatform(rel.assets || [], currentPlatform);
    downloadBtn.disabled = false;
    // Only the label: the button also holds its icon.
    const label = downloadBtn.querySelector(".dialog-button-label") || downloadBtn;
    label.textContent =
      (window.localization && window.localization.update_download) || "Download";
    downloadBtn.dataset.downloadUrl = asset ? asset.browser_download_url : rel.html_url;
  }

  // "Seen" gate only applies to auto-open of a newer release, not manual version-info clicks.
  if (info.isNewer && showDownload) {
    try { localStorage.setItem(SEEN_VERSION_KEY, info.remoteVersion); } catch (_) {}
  }

  showModal(modal);
}

async function openVersionInfoModal() {
  if (!latestReleaseInfo) {
    try {
      latestReleaseInfo = await invoke("gui_get_update_info");
    } catch (e) {
      console.error("Failed to fetch release info:", e);
    }
  }
  openUpdateModal({ showDownload: false });
}

// The line under the release title: its date, then the version, or
// "3.2.0 → 3.3.0" when it is newer than this build. A localized date and
// numbers only, so nothing here needs a translation.
function renderReleaseMeta(metaEl, info, rel) {
  if (!metaEl) return;
  metaEl.replaceChildren();
  const parts = [];
  const date = formatReleaseDate(rel.published_at);
  if (date) parts.push(document.createTextNode(date));

  const plain = (v) => String(v || "").replace(/^v/i, "");
  const remote = plain(info.remoteVersion || rel.tag_name);
  if (info.isNewer && info.localVersion && remote) {
    const version = document.createElement("span");
    const newer = document.createElement("span");
    newer.className = "is-new";
    newer.textContent = remote;
    version.append(`${plain(info.localVersion)} → `, newer);
    parts.push(version);
  } else if (remote) {
    parts.push(document.createTextNode(remote));
  }

  parts.forEach((part, i) => {
    if (i > 0) metaEl.append(" · ");
    metaEl.append(part);
  });
}

// The interface language as a locale for dates and numbers, rather than the
// system one.
function interfaceLocale() {
  const lang = localStorage.getItem("arnis-language") || navigator.language;
  // Arnis files Ukrainian under "ua"; the language tag is "uk".
  return lang === "ua" ? "uk" : lang;
}

function formatReleaseDate(iso) {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  try {
    return new Intl.DateTimeFormat(interfaceLocale(), { dateStyle: "long" }).format(date);
  } catch (_) {
    return date.toDateString();
  }
}

function closeUpdateModal() {
  const modal = document.getElementById("update-modal");
  if (modal) hideModal(modal);
}

// Where focus returns to once a dialog closes.
const modalReturnFocus = new WeakMap();

// Shows a dialog's backdrop and moves focus onto the dialog, so the keyboard
// acts on it rather than on whatever is behind.
function showModal(modal) {
  if (modal.style.display !== "flex") {
    modalReturnFocus.set(modal, document.activeElement);
  }
  modal.style.display = "flex";
  modal.style.justifyContent = "center";
  modal.style.alignItems = "center";
  const dialog = modal.querySelector(".dialog");
  if (dialog) dialog.focus({ preventScroll: true });
}

function hideModal(modal) {
  modal.style.display = "none";
  const back = modalReturnFocus.get(modal);
  modalReturnFocus.delete(modal);
  if (back && document.contains(back) && typeof back.focus === "function") {
    back.focus({ preventScroll: true });
  }
}

// A click on the backdrop closes the dialog, but only when the press started
// there too: selecting text inside and letting go outside must not close it.
function closeOnBackdrop(modal, close) {
  let pressedOnBackdrop = false;
  modal.addEventListener("pointerdown", (event) => {
    pressedOnBackdrop = event.target === modal;
  });
  modal.addEventListener("click", (event) => {
    if (pressedOnBackdrop && event.target === modal) close();
    pressedOnBackdrop = false;
  });
}

// License, version info and the 3D preview are read-only, so a click beside
// them may close them. The consent dialog is left alone: it wants an answer.
function initDialogs() {
  const license = document.getElementById("license-modal");
  const update = document.getElementById("update-modal");
  const preview3d = document.getElementById("preview3d-modal");
  if (license) closeOnBackdrop(license, () => window.closeLicense());
  if (update) closeOnBackdrop(update, closeUpdateModal);
  if (preview3d) closeOnBackdrop(preview3d, () => window.closePreview3D && window.closePreview3D());
}

function openUpdateInBrowser() {
  const url =
    (latestReleaseInfo && latestReleaseInfo.release && latestReleaseInfo.release.html_url) ||
    "https://github.com/louis-e/arnis/releases";
  openExternal(url);
}

function downloadLatestRelease() {
  const btn = document.getElementById("update-download-button");
  const url = btn && btn.dataset.downloadUrl;
  if (!url) return;
  openExternal(url);
}

function escapeHTMLLite(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

window.openUpdateModal = openUpdateModal;
window.openVersionInfoModal = openVersionInfoModal;
window.closeUpdateModal = closeUpdateModal;
window.openUpdateInBrowser = openUpdateInBrowser;
window.downloadLatestRelease = downloadLatestRelease;

// Earth on every start, so a Moon or Mars world stays a deliberate pick.
var selectedCelestialBody = 'earth';

// Earth-only options. Disabled rather than hidden, so it does not look like
// they were silently ignored.
const EARTH_ONLY_SETTINGS = [
  'generation-mode-select',
  'overture-toggle',
  'use-3d-toggle',
  'interior-toggle',
  'canopy-height-toggle',
  'legacy-trees-toggle',
  'scale-value-slider',
  'aws-only-elevation-toggle',
  'disable-height-limit-toggle',
  // Off Earth the body's own imagery is the only basemap, so neither the
  // Earth theme picker nor a custom Earth tile source has anything to act on.
  'tile-theme-select',
  'custom-tile-url'
];
const EARTH_ONLY_SEGMENTED = ['max-tree-size-group', 'signage-group'];

// A tile URL template Leaflet can actually fill in. Deliberately permissive
// about the host - the entire point is to reach something we do not know about -
// but the placeholders have to be there or every tile 404s.
function isValidTileTemplate(url) {
  if (!/^https?:\/\//i.test(url)) return false;
  return url.includes('{z}') && url.includes('{x}') && url.includes('{y}');
}

function getCustomTileUrl() {
  return (localStorage.getItem('customTileUrl') || '').trim();
}

function getMapillaryToken() {
  return (localStorage.getItem('mapillaryToken') || '').trim();
}

function getFacadeMode() {
  const stored = localStorage.getItem('facadeMode');
  if (stored === 'blocks') return 'blocks';
  // Anything else means the photographs: 'photos' itself, the 'paintings' and
  // 'paintings-v2' an earlier build saved for what are now the photo panels,
  // and nothing at all, which takes the default the same way the backend does.
  return 'photos';
}

// The photo panels are Java entities carried by a resource pack, so no other
// world format can show them. The stored choice is left alone so that going
// back to Java restores it; only what the backend is asked for changes.
function getEffectiveFacadeMode() {
  const mode = getFacadeMode();
  if (mode !== 'blocks' && selectedWorldFormat !== 'java') return 'blocks';
  return mode;
}

// One control decides where facades come from, so the two old switches are
// gone: they were mutually exclusive anyway and could both be off in two
// different ways. Java only, and the stored choice survives a trip through
// Bedrock so coming back restores it.
function getFacadeSource() {
  const v = localStorage.getItem('facadeSource');
  return v === 'preset' || v === 'mapillary' ? v : 'off';
}

// Only the presets are Java only. Mapillary falls back to block facades on
// Bedrock and Luanti, which is what the notice under the mode control says.
// The Moon and Mars are terrain only, so there is no wall to hang anything on
// and every source resolves to off there. As with the format gate, the stored
// choice is left alone so coming back to Earth restores it.
function getEffectiveFacadeSource() {
  const source = getFacadeSource();
  if (selectedCelestialBody !== 'earth') return 'off';
  if (source === 'preset' && selectedWorldFormat !== 'java') return 'off';
  return source;
}

function getFacadesEnabled() {
  return getEffectiveFacadeSource() === 'mapillary';
}

function getBuildingFacadesEnabled() {
  return getEffectiveFacadeSource() === 'preset';
}

function getFacadeDetail() {
  return localStorage.getItem('facadeDetail') === 'high' ? 'high' : 'standard';
}

// Every row under the source control only means something for one of the
// sources, so each is greyed by the source rather than by a switch of its own.
function refreshFacadeSourceRows() {
  const group = document.getElementById('facade-source-group');
  if (!group) return;
  const java = selectedWorldFormat === 'java';
  const earth = selectedCelestialBody === 'earth';
  const source = getEffectiveFacadeSource();

  // Off Earth the whole section is dead, the source picker included, so it is
  // greyed like the row it sits in rather than left looking live.
  group.classList.toggle('segmented-disabled', !earth);
  const sourceRow = group.closest('.settings-row');
  if (sourceRow) sourceRow.classList.toggle('settings-row-unavailable', !earth);
  group.querySelectorAll('.segment').forEach((btn) => {
    btn.disabled = !earth || (!java && btn.dataset.facadeSource === 'preset');
    btn.classList.toggle('active', btn.dataset.facadeSource === source);
  });

  // A row that belongs to another source is hidden: it has nothing to say
  // until that source is picked, and the whole Mapillary block greyed out
  // under Off only buried the rest of the page. A row that belongs to this
  // source but cannot act yet (no token, wrong format) is greyed instead, so
  // it is still there to explain itself.
  const grey = (id, shown, live) => {
    const el = document.getElementById(id);
    const row = el && el.closest('.settings-row');
    if (row) {
      row.style.display = shown ? '' : 'none';
      row.classList.toggle('settings-row-unavailable', !live);
    }
    if (el) {
      el.querySelectorAll('.segment, input, button').forEach((c) => {
        c.disabled = !live;
      });
      if (el.tagName === 'INPUT' || el.tagName === 'BUTTON') el.disabled = !live;
    }
  };
  const mapillary = source === 'mapillary';
  const token = !!getMapillaryToken();
  grey('mapillary-token', mapillary, mapillary);
  grey('facade-mode-group', mapillary, mapillary && token);
  // Panel resolution, so it means nothing where no panels are hung.
  grey('facade-detail-group', source !== 'off', source !== 'off' && java);
  grey('precompute-facades-button', mapillary, mapillary && token);

  const notice = document.getElementById('facade-java-only-notice');
  if (notice) notice.style.display = java ? 'none' : '';
}

// The facade rows react to the world format (panels are Java only), the on/off
// switch and the token. A row that cannot do anything is greyed out rather than
// left looking live, and the Precompute button reads the same facts.
function refreshFacadeRows() {
  const group = document.getElementById('facade-mode-group');
  const notice = document.getElementById('facade-java-only-notice');
  if (!group) return;

  const java = selectedWorldFormat === 'java';
  const earth = selectedCelestialBody === 'earth';
  const effective = getEffectiveFacadeMode();
  group.querySelectorAll('.segment').forEach((btn) => {
    const panels = btn.dataset.facadeMode !== 'blocks';
    btn.disabled = panels && !java;
    btn.classList.toggle('active', btn.dataset.facadeMode === effective);
  });
  // Off Earth nothing builds facades at all, so the format gate has nothing
  // left to explain.
  if (notice) notice.style.display = java || !earth ? 'none' : '';
  refreshFacadeSourceRows();
  refreshPrecomputeButton();
}

// The URL field is only meaningful for the Custom theme, so it is hidden
// rather than disabled for every other one.
function refreshCustomSourceRow() {
  const select = document.getElementById('tile-theme-select');
  const row = document.getElementById('custom-tile-row');
  if (!select || !row) return;
  row.style.display = select.value === 'custom' ? '' : 'none';
}

// The map iframe is addressed by class, never by its src attribute. Manual
// bbox entry used to rewrite src to "maps.html#lat,lng,lat,lng", after which
// every iframe[src="maps.html"] lookup silently matched nothing and the map
// theme and celestial body messages were dropped for the rest of the session.
function getMapFrame() {
  return document.querySelector('.map-container');
}

// The map owns the toggle, but an iframe reload restarts it on Earth, so the
// parent's value has to be pushed back.
function pushBodyToMap() {
  const mapIframe = getMapFrame();
  if (mapIframe && mapIframe.contentWindow) {
    mapIframe.contentWindow.postMessage(
      { type: 'changeBody', body: selectedCelestialBody },
      '*'
    );
  }
}

function setCelestialBody(body) {
  selectedCelestialBody = (body === 'moon' || body === 'mars') ? body : 'earth';
  const off = selectedCelestialBody !== 'earth';

  const markRow = (el) => {
    const row = el && el.closest('.settings-row');
    if (row) row.classList.toggle('settings-row-unavailable', off);
  };

  EARTH_ONLY_SETTINGS.forEach((id) => {
    const el = document.getElementById(id);
    if (!el) return;
    el.disabled = off;
    markRow(el);
  });

  EARTH_ONLY_SEGMENTED.forEach((id) => {
    const group = document.getElementById(id);
    if (!group) return;
    group.classList.toggle('segmented-disabled', off);
    markRow(group);
  });

  // Gated on more than the body, so they cannot simply follow the earth-only
  // loop: the height-limit pack is also format-gated and the facade rows read
  // the world format and the stored source too.
  refreshHeightLimitRow();
  refreshFacadeRows();
  refreshOneWorldState();

  // A default, not a lock. Slider units are clock minutes: 0 midnight, 720 noon.
  const timeSlider = document.getElementById('world-time-slider');
  if (timeSlider) {
    timeSlider.value = off ? 0 : 720;
    timeSlider.dispatchEvent(new Event('input', { bubbles: true }));
  }

  // The cached preview describes a world on the body we just left. The map
  // clears its own overlay in changeBody; this drops the parent's copy so a
  // later ready event cannot put a stale one back.
  currentWorldMapData = null;

  // Warnings are per body, so the current selection may read differently now.
  refreshBboxSelectionInfo();
}

// Function to register the event listener for bbox updates from iframe
function registerMessageEvent() {
  window.addEventListener('message', function (event) {
    const bboxText = event.data.bboxText;

    // Typed messages are handled below; only untyped bboxText messages are
    // selection updates (typed ones carrying coordinates must not be).
    if (bboxText && !event.data.type) {
      if (!run && runMap && bboxText !== runMap.src) {
        runMap = null;
        postRunMap();
      }
      console.log("Updated BBOX Coordinates:", bboxText);
      displayBboxInfoText(bboxText);
    }

    // "Retry this cell" on a failed or stopped cell of the map
    if (event.data && event.data.type === 'retryPiece') {
      retryCells([event.data.piece]);
    }

    // World toggled on the map toolbar
    if (event.data && event.data.type === 'bodyChanged') {
      setCelestialBody(event.data.body);
    }

    // Handle angle measurement from the map polyline tool
    if (event.data && event.data.type === 'angleMeasured') {
      // A One World is aligned to real coordinates; the tool is disabled on
      // the map too, this only covers a measurement already in flight.
      if (isOneWorldEnabled()) return;
      var angle = event.data.angle;
      var rotationInput = document.getElementById("rotation-angle-input");
      if (rotationInput) {
        var clamped = Math.min(Math.max(angle, -90), 90);
        rotationInput.value = clamped.toFixed(2);
        // Also trigger the rotation preview update on the map
        var mapFrame = document.querySelector('.map-container');
        if (mapFrame && mapFrame.contentWindow) {
          mapFrame.contentWindow.postMessage({
            type: 'rotatePreview',
            angle: clamped
          }, '*');
        }
      }
    }
  });
}

// --- Self-calibrating, phase-aware ETA ------------------------------------
// The single 0-100% progress is non-linear in time: it has fixed phase
// breakpoints and the post-70% tail is ~instant under stream-to-disk but a real
// save otherwise. So we model three time-bands, extrapolate the CURRENT band to
// its own end from a least-squares rate, and budget the remaining bands via
// per-regime time weights calibrated to this run. The backend tells us the
// streaming regime via an optional `streaming` field (absent => non-streaming).
// Starts once generation begins (the first band's lo: 20%, downloads done) and
// ticks down once a second so it reads like a live countdown.
const ETA_WINDOW_MS = 16000; // sliding window for the rate (wider = steadier)
const ETA_MIN_MS = 700; // min window span before trusting a rate
const ETA_MIN_SAMPLES = 4; // keep at least this many samples in the window
const ETA_STALL_MS = 1500; // progress flat this long => freeze belief, keep ticking
const ETA_MAX_S = 24 * 3600; // ignore absurd extrapolations
const ETA_A_DOWN = 0.28; // smoothing when the estimate falls (gentle)
const ETA_A_UP = 0.1; // smoothing when it rises (resist, stay calm)
const ETA_RISE_ABS = 2; // shown rises by at most max(2s, 10%) per update
const ETA_RISE_FRAC = 0.1; // => smooth, monotonic-feeling countdown

// Progress bands [lo, hi) and per-regime RELATIVE time weights (not % widths).
// Measured on Heidelberg 1/2.5/5/10 km runs (terrain + land cover + Overture):
// the finalize tail (map item, signage map tiles, world settings) is as long as
// the region write on small areas and several times longer on large ones, so it
// gets its own band rather than hiding behind the last save percent.
const ETA_PHASES = [
  { id: "terrain", lo: 20, hi: 70 },
  { id: "ground", lo: 70, hi: 90 },
  { id: "save", lo: 90, hi: 97 },
  { id: "finalize", lo: 97, hi: 100 },
];
const ETA_WPRIOR = {
  nonStreaming: [37, 2, 11, 20],
  streaming: [60, 0.3, 0.5, 3.0],
};
// A run in pieces reports one 0-100% for the whole job, each piece an equal share (the
// coordinator's `report` in scale/mod.rs), so it is one band from the start.
const ETA_PIECED_PHASES = [{ id: "pieces", lo: 1, hi: 100 }];
// Signage map tiles are what makes the finalize band long, and they are Java-only.
// Without them the tail is just the map item and level.dat settings.
const ETA_WFINALIZE_NO_SIGNAGE = 1.5;

let eta = null;
// Set by the coordinator's first "Building pieces..." line, cleared per run.
let etaPieced = false;
const etaPhases = () => (etaPieced ? ETA_PIECED_PHASES : ETA_PHASES);
// Set from the generate handler; decides the finalize weight for the next run.
let etaSignageExpected = true;

function setEtaSignageExpected(expected) {
  etaSignageExpected = !!expected;
}

// Copy of the regime prior with the finalize weight adjusted for this run.
function etaWeightsFor(streaming) {
  if (etaPieced) return [1];
  const w = (streaming ? ETA_WPRIOR.streaming : ETA_WPRIOR.nonStreaming).slice();
  if (!etaSignageExpected) w[3] = ETA_WFINALIZE_NO_SIGNAGE;
  return w;
}

function etaPhaseIdx(p) {
  const phases = etaPhases();
  for (let i = 0; i < phases.length; i++) if (p < phases[i].hi) return i;
  return phases.length - 1;
}

// Least-squares slope of progress over the window -> %/sec (null if not rising).
function etaLsRate(s) {
  const n = s.length;
  if (n < 2) return null;
  let st = 0, sp = 0;
  for (const x of s) { st += x.t; sp += x.p; }
  const mt = st / n, mp = sp / n;
  let num = 0, den = 0;
  for (const x of s) { const d = x.t - mt; num += d * (x.p - mp); den += d * d; }
  if (den <= 0) return null;
  const slope = num / den;
  return slope > 0 ? slope * 1000 : null;
}

function resetEta() {
  if (eta && eta.tickHandle) clearInterval(eta.tickHandle);
  eta = null;
  const el = document.getElementById("progress-eta");
  if (el) {
    el.classList.remove("visible");
    el.textContent = "";
    el.removeAttribute("aria-label");
  }
}

function formatEtaDuration(sec) {
  sec = Math.max(0, Math.round(sec));
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  if (m < 60) return s ? `${m}m ${s}s` : `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function renderEta() {
  const el = document.getElementById("progress-eta");
  if (!el) return;
  // Show nothing until we actually have an estimate (no "…" placeholder).
  if (!eta || eta.shown == null) {
    el.classList.remove("visible");
    el.textContent = "";
    el.removeAttribute("aria-label");
    return;
  }
  // Floor at 1s so it never reads "0s" before Done.
  const value = formatEtaDuration(Math.max(1, eta.shown));
  el.textContent = value; // duration only on the bar; no "~" prefix
  // Screen-reader context (the visible pill is just the duration). Hardcoded
  // English to match the rest of the progress messages, which aren't localized.
  el.setAttribute("aria-label", `Time remaining: ${value}`);
  el.classList.add("visible");
}

// Two-layer display: `est` is the belief, `shown` follows it but falls freely
// and rises slowly, so the countdown never jumps upward jarringly.
function etaReconcile() {
  if (!eta || eta.est == null) return;
  if (eta.shown == null || eta.est <= eta.shown) eta.shown = eta.est;
  else
    eta.shown = Math.min(
      eta.est,
      eta.shown + Math.max(ETA_RISE_ABS, eta.shown * ETA_RISE_FRAC)
    );
}

// Counts the value down between progress events (and through the 70% stall).
function etaTick() {
  if (!eta || eta.est == null) return;
  const now = performance.now();
  const dt = (now - eta.lastTickAt) / 1000;
  eta.lastTickAt = now;
  eta.est = Math.max(0, eta.est - dt);
  if (eta.shown != null) eta.shown = Math.max(0, eta.shown - dt);
  etaReconcile();
  renderEta();
}

function updateEta(progress, streaming) {
  // Reset before the generation phases and once finished / on a new run.
  const phases = etaPhases();
  if (progress < phases[0].lo || progress >= 100) {
    resetEta();
    return;
  }
  const now = performance.now();
  if (!eta) {
    eta = {
      streamingKnown: false, wprior: etaWeightsFor(false), phaseIdx: -1,
      phaseStartT: now, phaseStartProgress: null, movedInPhase: false,
      samples: [], doneSec: 0, doneWeight: 0, est: null, shown: null,
      lastTickAt: now, lastProgress: null, lastIncreaseAt: now, tickHandle: null,
    };
  }
  if (streaming != null && !eta.streamingKnown) {
    eta.streamingKnown = true;
    eta.wprior = etaWeightsFor(streaming);
  }

  const idx = etaPhaseIdx(progress), ph = phases[idx];
  if (idx !== eta.phaseIdx) {
    // Bank the real duration (incl. any stall) of the phase we just left.
    if (eta.phaseIdx >= 0) {
      eta.doneSec += (now - eta.phaseStartT) / 1000;
      eta.doneWeight += eta.wprior[eta.phaseIdx];
      for (let k = eta.phaseIdx + 1; k < idx; k++) eta.doneWeight += eta.wprior[k];
    }
    eta.phaseIdx = idx;
    eta.phaseStartT = now;
    eta.samples = [];
    eta.phaseStartProgress = progress;
    eta.movedInPhase = false;
  }

  if (eta.lastProgress == null || progress > eta.lastProgress) eta.lastIncreaseAt = now;
  eta.lastProgress = progress;
  const stalled = now - eta.lastIncreaseAt > ETA_STALL_MS;

  eta.samples.push({ t: now, p: progress });
  // Drop the flat phase-start hold (e.g. precompute sitting at 25%) the first
  // time progress actually moves, so the rate reflects real work, not setup.
  if (!eta.movedInPhase && eta.phaseStartProgress != null && progress > eta.phaseStartProgress) {
    eta.samples = [{ t: now, p: progress }];
    eta.movedInPhase = true;
  }
  const cut = now - ETA_WINDOW_MS;
  while (eta.samples.length > ETA_MIN_SAMPLES && eta.samples[0].t < cut) eta.samples.shift();
  const span = now - eta.samples[0].t;
  const rate = !stalled && span >= ETA_MIN_MS ? etaLsRate(eta.samples) : null;

  const phaseElapsed = (now - eta.phaseStartT) / 1000;
  let G = eta.doneWeight > 0 ? eta.doneSec / eta.doneWeight : null; // sec per prior-unit
  let remCur = null;
  if (rate) remCur = (ph.hi - progress) / rate;
  else if (G != null) remCur = ((ph.hi - progress) / (ph.hi - ph.lo)) * eta.wprior[idx] * G;
  if (G == null && remCur != null) G = (phaseElapsed + remCur) / eta.wprior[idx];
  if (G != null) G = Math.min(1000, Math.max(0.02, G));

  let raw = null;
  if (remCur != null) {
    // In the last band this is just the measured rate; earlier ones budget the rest.
    raw = Math.max(0, remCur);
    if (G != null) for (let j = idx + 1; j < phases.length; j++) raw += eta.wprior[j] * G;
  }

  if (!stalled && raw != null && isFinite(raw) && raw <= ETA_MAX_S) {
    const a = eta.est == null || raw <= eta.est ? ETA_A_DOWN : ETA_A_UP;
    eta.est = eta.est == null ? raw : a * raw + (1 - a) * eta.est;
    eta.lastTickAt = now;
  }
  if (eta.est != null && !eta.tickHandle) {
    eta.lastTickAt = now;
    eta.tickHandle = setInterval(etaTick, 1000);
  }
  etaReconcile();
  renderEta();
}

// What the status line says between the click and the backend's first real
// progress event. Kept in a constant because the abort paths have to be able to
// recognize it as still-unclaimed and take it back down.
const STARTING_MESSAGE = "Starting...";

// The bar and the status line only ever move on a `progress-update` event, and
// the first one is a long way from the click: a world folder gets created, a
// spawn point written into level.dat, a datapack possibly installed, disk space
// probed and a session lock taken, all before the download that emits 1%. Until
// then the previous run's green "Done!" and full bar sit there, which reads as
// the button having done nothing. So claim both here, at the moment the click
// is accepted, and let the first real update take over from 0%.
function resetProgressUi(message) {
  const bar = document.getElementById("progress-bar");
  const info = document.getElementById("progress-info");
  const detail = document.getElementById("progress-detail");
  if (bar) bar.style.width = "0%";
  if (detail) detail.textContent = "0%";
  if (info) {
    info.textContent = message;
    info.style.color = "#ececec";
  }
  resetEta();
  etaPieced = false;
  runStats = { startedAt: performance.now(), pieces: selectionPieces(), done: 0, estimate: runEstimate() };
  renderRunStats();
}

// The line under the bar: before a run, runEstimate() for the selection;
// during one, the time since the click, the pieces done and the estimated
// size. The time left stays on the bar (#progress-eta).
let runStats = null; // { startedAt, pieces: { n, w } | null, done, estimate }

function estimateSizePart(e) {
  return e && e.mb != null
    ? [['hard-drive', oneWorldText('run_estimate_size', 'On disk: ~{size}', { size: formatEstimateSize(e.mb) })]] : [];
}

// The selection's estimate: its size on disk and the time to build it.
function estimateParts() {
  const e = runEstimate();
  return estimateSizePart(e).concat(e
    ? [['hourglass', oneWorldText('run_estimate_time', 'Build time: {time}', { time: formatEstimateTime(e.lo, e.hi) })]] : []);
}

function runStatParts(withEta) {
  const parts = [];
  if (!runStats) return estimateParts();
  parts.push(['clock', oneWorldText('run_elapsed', 'Elapsed: {time}', {
    time: formatEtaDuration((performance.now() - runStats.startedAt) / 1000),
  })]);
  if (withEta && eta && eta.shown != null) {
    parts.push(['hourglass', oneWorldText('run_eta', 'Remaining: {time}', { time: formatEtaDuration(Math.max(1, eta.shown)) })]);
  }
  const p = runStats.pieces;
  if (p) {
    let text = oneWorldText('run_pieces', 'Pieces: {done}/{n}', { done: runStats.done, n: p.n });
    if (p.w) text += ' · ' + oneWorldText('run_workers', 'Workers: {w}', { w: p.w });
    parts.push(['layers', text]);
  }
  parts.push(...estimateSizePart(runStats.estimate));
  return parts;
}

function renderRunStats() {
  fillRunStats(document.getElementById('run-stats'), runStatParts(false));
  renderOfflinePanel();
  syncRetryRow();
}

/* The offline panel under the run controls (Extra Features on): one summary
   bar for what the selection still needs (the OSM Data Source transfer panel,
   so a download or bake runs in the very same bar), and the Download Plan's
   rows under a collapsed Details. Both are the settings components, moved
   here by placeOfflinePanel, so one implementation serves both places. */
function placeOfflinePanel() {
  const main = extraFeaturesOn();
  const body = document.getElementById('data-plan-body');
  const transfer = document.getElementById('transfer-panel');
  document.getElementById('data-plan-moved').hidden = !main;
  document.getElementById('offline-first-toggle').hidden = !main || !!run;
  if (main) {
    document.getElementById('offline-summary').appendChild(transfer);
    document.getElementById('offline-details-slot').appendChild(body);
  } else {
    document.getElementById('data-plan-row').appendChild(body);
    body.insertBefore(transfer, document.getElementById('bake-threads'));
    document.getElementById('offline-download-button').hidden = true;
    if (!transferJob) transfer.style.display = 'none';
  }
  renderOfflinePanel();
}

// What the plan still needs: bytes to download (countries to bake included),
// bytes a Region Download bake adds, and the bytes already on disk.
function offlineNeed() {
  if (!dataPlan) return null;
  let download = 0, cached = 0;
  for (const item of dataPlan.items) {
    cached += item.cached_bytes || 0;
    if (item.cached < item.total && item.missing_bytes) download += item.missing_bytes;
  }
  const e = dataPlan.extract;
  const bake = e && e.name && e.bake_bytes == null && e.bake_estimate ? e.bake_estimate : 0;
  const todo = preparePlan && document.getElementById('osm-source-select').value === 'local'
    ? preparePlan.extracts.filter((x) => !x.baked) : [];
  const countries = todo.reduce((a, x) => a + x.bytes, 0);
  return { download: download + countries, bake, cached, total: cached + download + countries + bake };
}

// The rate the time left is worked out at: the last download's, or a typical line.
// ponytail: one figure for every source; per-source rates if the guess misleads.
let offlineRate = 5e6;

// Idle, the summary bar shows the share already cached and what is left.
function renderOfflineSummary() {
  if (transferJob || !extraFeaturesOn()) return;
  const t = oneWorldText;
  const panel = document.getElementById('transfer-panel');
  const need = offlineNeed();
  const left = need ? need.download + need.bake : 0;
  panel.style.display = '';
  panel.classList.add('is-ended');
  panel.classList.toggle('is-done', !!need && left === 0);
  document.getElementById('transfer-stop-button').style.display = 'none';
  const now = document.getElementById('offline-download-button');
  now.hidden = false;
  now.disabled = !need || left === 0 || generationButtonEnabled === false;
  document.getElementById('transfer-stage').textContent = need && left === 0
    ? t('offline_cached_all', 'Everything is cached') : t('offline_missing', "Download & bake what's missing");
  setTransferBar(!need ? 0 : left === 0 ? 100 : 100 * need.cached / Math.max(1, need.total));
  const parts = [];
  if (left > 0) {
    parts.push(t('offline_need', '~{size} · about {time}', {
      size: formatPlanBytes(left), time: formatClock(need.download / offlineRate),
    }));
  }
  parts.push(...estimateParts().map((part) => part[1]));
  document.getElementById('transfer-detail').textContent = parts.join(' · ');
}

function renderOfflinePanel() {
  const panel = document.getElementById('offline-panel');
  panel.hidden = !(extraFeaturesOn() && selectedBBox);
  if (panel.hidden) return;
  renderOfflineSummary();
}

function fillRunStats(el, parts) {
  if (!el) return;
  const key = JSON.stringify(parts);
  if (key === el.dataset.key) return;
  el.dataset.key = key;
  el.replaceChildren(...parts.map(([icon, text]) => {
    const span = document.createElement('span');
    span.className = 'run-stat';
    span.innerHTML = `<svg class="icon" aria-hidden="true"><use href="#i-${icon}"></use></svg>`;
    span.append(text);
    return span;
  }));
}

// For the mini panel (mini.js): whether a run is going, and its line with the time left.
window.arnisRunRunning = () => !!runStats;
window.arnisFillRunStats = (el) => fillRunStats(el, runStatParts(true));

// Function to set up the progress bar listener
function setupProgressListener() {
  const progressBar = document.getElementById("progress-bar");
  const progressInfo = document.getElementById("progress-info");
  const progressDetail = document.getElementById("progress-detail");

  setInterval(renderRunStats, 1000);
  window.__TAURI__.event.listen("progress-update", (event) => {
    const { progress, message, streaming } = event.payload;
    if (event.payload.piece) onRunPiece(event.payload.piece);
    else if (progress >= 0) onRunPercent(progress);
    // "Building pieces... 3/16 done": the coordinator's count, and a one-band ETA.
    if (message && message.startsWith("Building pieces")) {
      // Anything the four-band model counted before this is not the job's.
      if (!etaPieced) resetEta();
      etaPieced = true;
      const m = message.match(/(\d+)\/(\d+) done/);
      if (runStats && m) {
        runStats.done = +m[1];
        if (!runStats.pieces || runStats.pieces.n !== +m[2]) runStats.pieces = { n: +m[2], w: runStats.pieces?.w };
      }
      renderRunStats();
    }

    if (progress != -1) {
      progressBar.style.width = `${progress}%`;
      progressDetail.textContent = `${Math.round(progress)}%`;
      updateEta(progress, streaming);
    }

    if (message != "") {
      progressInfo.textContent = message;

      if (message.startsWith("Error!") || message.startsWith("Done!")) {
        runStats = null;
        renderRunStats();
        endRunControls(message);
      }
      if (message.startsWith("Error!")) {
        progressInfo.style.color = "#fa7878";
        setGenerationButtonEnabled(true);
        window.arnisPreview3D?.setGenerationRunning(false);
        if (lastRunOneWorld) {
          // The world keeps its name; the status line carries the reason.
          setWorldNameLabel(isOneWorldEnabled() ? oneWorldDisplayName() : lastRunWorldName);
          if (isOneWorldEnabled()) setOneWorldStatus(message.replace(/^Error!\s*/, ''), 'error');
        } else {
          setWorldNameLabel("");
        }
        resetEta();
      } else if (message.startsWith("Done!")) {
        progressInfo.style.color = "#7bd864";
        setGenerationButtonEnabled(true);
        window.arnisPreview3D?.setGenerationRunning(false);
        resetEta();
        // The world just grew by one area: reload its status and overlays.
        if (lastRunOneWorld) {
          oneWorldOverlayKey = null;
          refreshOneWorldState();
        }
        // A generation just built facades into the same cache, so the preview
        // has something new to say.
        window.arnisPreview3D?.refreshFacades();
      } else {
        progressInfo.style.color = "#ececec";
      }
      // The facade pipeline reports its stages here whichever job is driving it.
      notePrecomputeStage(message);
      // A finished download (or a failed one) changed what the caches hold.
      if (message.startsWith("Done!") || message.startsWith("Error!")) refreshDataPlan(true);
    }
    onTransferProgress(progress, message, event.payload.transfer);
    // A stopped download is over too, with nothing to colour.
    if (message.startsWith("Stopped.")) {
      progressInfo.style.color = "#ececec";
      runStats = null;
      renderRunStats();
      endRunControls(message);
      setGenerationButtonEnabled(true);
      resetEta();
      refreshDataPlan(true);
    }
    if (transferJob === 'prewarm' && /^(Done!|Error!|Stopped\.)/.test(message)) {
      transferEnd(message, message.startsWith("Done!"));
      refreshStorage();
      if (generateAfterDownload) {
        generateAfterDownload = false;
        // After the download has let go of the process; ponytail: a fixed
        // wait, an event from gui_start_generation's end if it ever falls short.
        if (message.startsWith("Done!")) setTimeout(() => startGeneration({ afterDownload: true }), 1000);
      }
    }
  });

  // Listen for the finalized world name (Java adds the localized area suffix
  // during generation; Bedrock derives the name from the area up-front).
  window.__TAURI__.event.listen("world-name-update", (event) => {
    if (typeof event.payload === 'string') {
      setWorldNameLabel(event.payload);
    }
  });

  // Listen for map preview ready event from backend
  window.__TAURI__.event.listen("map-preview-ready", () => {
    console.log("Map preview ready event received");
    showWorldPreviewButton();
  });

  // Listen for show-in-folder event to reveal the generated world in the file explorer
  window.__TAURI__.event.listen("show-in-folder", async (event) => {
    const filePath = event.payload;
    try {
      await invoke("gui_show_in_folder", { path: filePath });
    } catch (error) {
      console.error("Failed to show file in folder:", error);
    }
  });
}

// Easter eggs
function showEasterEggAnimal() {
  const img = document.getElementById('secret-parrot');
  img.src = './images/parrot.gif';
  img.style.display = 'inline';
}

function initEasterEggs() {
  // 1 in 50 chance at startup
  if (Math.random() < 1 / 50) {
    showEasterEggAnimal();
  }

  // 5 rapid clicks on progress bar
  const progressBar = document.querySelector('.progress-bar-container');
  let clicks = [];
  progressBar.addEventListener('click', () => {
    const now = Date.now();
    clicks.push(now);
    clicks = clicks.filter(t => now - t < 1500);
    if (clicks.length >= 5) {
      showEasterEggAnimal();
      clicks = [];
    }
  });
}

// Language implied by the browser, ignoring any stored preference.
function detectBrowserLanguage(availableOptions) {
  const currentLang = navigator.language || 'en';
  if (availableOptions.includes(currentLang)) return currentLang;
  const base = currentLang.split('-')[0];
  if (availableOptions.includes(base)) return base;
  return 'en';
}

// Gives the settings store the save path defaults. Not awaited, since startup
// must not block on a filesystem probe; on failure the revert stays hidden.
function resolveDefaultSavePath() {
  const resolve = (command, name) => {
    Promise.resolve()
      .then(() => invoke(command))
      .then((detected) => {
        if (typeof detected === 'string' && detected) {
          setDynamicDefault(name, detected);
        }
      })
      .catch(() => {
        // No detectable default, so that row keeps no revert button.
      });
  };

  resolve('gui_get_default_save_path', 'savePath');
  resolve('gui_get_default_bedrock_save_path', 'bedrockSavePath');
  resolve('gui_get_default_luanti_save_path', 'luantiSavePath');
}

function initSettings() {
  // Settings
  const settingsModal = document.getElementById("settings-modal");
  const slider = document.getElementById("scale-value-slider");
  const sliderValue = document.getElementById("slider-value");

  // Where focus goes back to once the page closes.
  let focusBeforeSettings = null;

  // Open the settings page
  function openSettings() {
    focusBeforeSettings = document.activeElement;
    settingsModal.style.display = "flex";
    syncSettingsLayout();
    // Focus moves onto the page, so Tab and the arrow keys act on it rather
    // than on the map behind it.
    settingsModal.focus({ preventScroll: true });
    // The caches grow with every generation, so the number the panel shows
    // has to be read when the panel opens; measuring it once at startup left
    // it stale for the whole session.
    refreshCacheSize();
    // Same for the plan: a run since the last look may have filled the caches.
    refreshDataPlan(true);
    // Auto's realm follows the selection.
    refreshOptionPreviews();
  }

  // Close the settings page
  function closeSettings() {
    settingsModal.style.display = "none";
    // Webview teardown events are not guaranteed, so commit here.
    flushSettingsStore();
    cancelSettingsResetConfirm();
    if (focusBeforeSettings && typeof focusBeforeSettings.focus === "function") {
      focusBeforeSettings.focus({ preventScroll: true });
    }
    focusBeforeSettings = null;
  }

  // Escape closes the topmost dialog only. License and version info open on
  // top of the settings page, so closing them returns to it.
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;

    const licenseModal = document.getElementById("license-modal");
    const updateModal = document.getElementById("update-modal");
    const licenseOpen = licenseModal && licenseModal.style.display === "flex";
    const updateOpen = updateModal && updateModal.style.display === "flex";

    if (licenseOpen) closeLicense();
    if (updateOpen) closeUpdateModal();
    if (!licenseOpen && !updateOpen && settingsModal.style.display === "flex") {
      closeSettings();
    }
  });

  window.openSettings = openSettings;
  window.closeSettings = closeSettings;

  // Mirrors OBJECT_SKIP_SCALE in src/args.rs
  const OBJECT_SKIP_SCALE = 0.3;
  const scaleObjectsNote = document.getElementById("scale-objects-note");

  function refreshScaleDisplay() {
    const value = parseFloat(slider.value);
    sliderValue.textContent = value.toFixed(2);
    if (scaleObjectsNote) {
      scaleObjectsNote.style.display = value < OBJECT_SKIP_SCALE ? "" : "none";
    }
  }

  slider.addEventListener("input", refreshScaleDisplay);
  slider.addEventListener("input", refreshBboxSelectionInfo);
  // Double-click to reset world scale to default (1.00).
  // Assigning .value fires no event, so dispatch them for the label and store.
  slider.addEventListener("dblclick", () => {
    slider.value = 1;
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
  });
  refreshScaleDisplay();

  const heightSlider = document.getElementById("height-multiplier-slider");
  const heightValue = document.getElementById("height-multiplier-value");
  const refreshHeightDisplay = () => {
    heightValue.textContent = parseFloat(heightSlider.value).toFixed(2) + "\u00d7";
  };
  heightSlider.addEventListener("input", refreshHeightDisplay);
  heightSlider.addEventListener("dblclick", () => {
    heightSlider.value = 1;
    heightSlider.dispatchEvent(new Event("input", { bubbles: true }));
    heightSlider.dispatchEvent(new Event("change", { bubbles: true }));
  });
  refreshHeightDisplay();

  // Game mode segmented control
  const gamemodeGroup = document.getElementById("gamemode-group");
  gamemodeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      gamemodeGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // World type segmented control
  const worldTypeGroup = document.getElementById("world-type-group");
  worldTypeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      worldTypeGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // Signage segmented control
  const signageGroup = document.getElementById("signage-group");
  signageGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      signageGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // A reloaded map comes back on Earth; restore whatever was picked.
  const bodyMapFrame = getMapFrame();
  if (bodyMapFrame) bodyMapFrame.addEventListener('load', pushBodyToMap);

  // Max tree size segmented control
  const maxTreeSizeGroup = document.getElementById("max-tree-size-group");
  maxTreeSizeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      maxTreeSizeGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // World time slider (clock minutes 00:00-23:50; converted to ticks on submit)
  const timeSlider = document.getElementById("world-time-slider");
  const timeValue = document.getElementById("world-time-value");
  function formatClock(minutes) {
    if (minutes >= 1440) return "24:00";
    const h = String(Math.floor(minutes / 60)).padStart(2, "0");
    const m = String(minutes % 60).padStart(2, "0");
    return `${h}:${m}`;
  }
  timeSlider.addEventListener("input", () => {
    timeValue.textContent = formatClock(parseInt(timeSlider.value, 10));
  });
  timeSlider.addEventListener("dblclick", () => {
    timeSlider.value = 720;
    timeSlider.dispatchEvent(new Event("input", { bubbles: true }));
    timeSlider.dispatchEvent(new Event("change", { bubbles: true }));
  });

  // Rotation angle input
  const rotationInput = document.getElementById("rotation-angle-input");

  function updateRotation(val) {
    if (isNaN(val)) val = 0;
    val = Math.min(Math.max(val, -90), 90);
    rotationInput.value = val.toFixed(2);
    // The bbox handlers set this from code, which fires no input event.
    refreshSettingsState();
    // Tell the map iframe to update the rotation mask overlay
    const mapFrame = document.querySelector('.map-container');
    if (mapFrame && mapFrame.contentWindow) {
      mapFrame.contentWindow.postMessage({
        type: 'rotatePreview',
        angle: val
      }, '*');
    }
  }
  rotationInput.addEventListener("input", () => {
    updateRotation(parseFloat(rotationInput.value));
  });
  rotationInput.addEventListener("change", () => {
    updateRotation(parseFloat(rotationInput.value));
  });
  window.updateRotation = updateRotation;

  // World format toggle (Java/Bedrock/Luanti)
  initWorldFormatToggle();

  // Custom world name editor (Java only), gated by its Settings toggle
  initCustomWorldNameToggle();

  // One World: a persistent world every area extends (Java only)
  initOneWorld();

  // Save path setting
  initSavePathSetting();

  // Language selector
  const languageSelect = document.getElementById("language-select");
  const availableOptions = Array.from(languageSelect.options).map(opt => opt.value);

  // The default here is the browser language, not an HTML attribute.
  setDynamicDefault('language', detectBrowserLanguage(availableOptions));

  // Check for saved language preference first
  const savedLanguage = localStorage.getItem('arnis-language');
  let languageToSet;

  if (savedLanguage && availableOptions.includes(savedLanguage)) {
    // Use saved language if it exists and is available
    languageToSet = savedLanguage;
  } else {
    // Otherwise use browser language
    languageToSet = detectBrowserLanguage(availableOptions);
  }

  languageSelect.value = languageToSet;

  // Handle language change
  languageSelect.addEventListener("change", async () => {
    const selectedLanguage = languageSelect.value;

    // Store the selected language in localStorage for persistence
    localStorage.setItem('arnis-language', selectedLanguage);

    // Reload localization with the new language
    const localization = await fetchLanguage(selectedLanguage);
    await applyLocalization(localization);

    // Restore correct format toggle state after localization
    updateFormatToggleUI(selectedWorldFormat);
  });

  // Tile theme selector
  const tileThemeSelect = document.getElementById("tile-theme-select");

  // Load saved tile theme preference
  const savedTileTheme = localStorage.getItem('selectedTileTheme') || 'osm';
  tileThemeSelect.value = savedTileTheme;

  // Handle tile theme change
  tileThemeSelect.addEventListener("change", () => {
    const selectedTheme = tileThemeSelect.value;

    // Store the selected theme in localStorage for persistence
    localStorage.setItem('selectedTileTheme', selectedTheme);
    refreshCustomSourceRow();

    // Send message to map iframe to change tile theme
    const mapIframe = getMapFrame();
    if (mapIframe && mapIframe.contentWindow) {
      mapIframe.contentWindow.postMessage({
        type: 'changeTileTheme',
        theme: selectedTheme
      }, '*');
    }
  });

  // Custom map source, the field behind the Custom theme. It exists because a
  // network that blocks every built-in provider turns the fallback chain into a
  // slower route to the same blank map (see issues #1222, #1298, #1299).
  const customTileInput = document.getElementById("custom-tile-url");
  customTileInput.value = getCustomTileUrl();

  // Mapillary token. Kept in localStorage like the save paths so it survives a
  // restart; it is a per-user API credential, so it is never written to a log
  // or sent anywhere except the backend that fetches with it.
  const mapillaryTokenInput = document.getElementById("mapillary-token");
  mapillaryTokenInput.value = getMapillaryToken();
  mapillaryTokenInput.addEventListener("change", () => {
    const raw = mapillaryTokenInput.value.trim();
    if (raw) {
      localStorage.setItem('mapillaryToken', raw);
    } else {
      localStorage.removeItem('mapillaryToken');
    }
    refreshFacadeRows();
  });

  // The source control, and the detail beside it. Each keeps its own key so
  // the rest of main.js can read it without the store, and is registered in
  // settings-store.js as well so the panel's Revert and Reset reach it.
  const segmented = (id, storageKey, dataAttr, after) => {
    const group = document.getElementById(id);
    if (!group) return;
    group.querySelectorAll(".segment").forEach((btn) => {
      btn.addEventListener("click", () => {
        // A disabled segment is still clicked by settings-store.js when it
        // restores or reverts, and refusing that would lose a choice made on
        // Java the moment the format changed.
        localStorage.setItem(storageKey, btn.dataset[dataAttr]);
        group.querySelectorAll(".segment").forEach((b) => {
          b.classList.toggle("active", b === btn);
        });
        if (after) after();
      });
    });
  };
  segmented("facade-source-group", "facadeSource", "facadeSource", refreshFacadeRows);
  segmented("facade-detail-group", "facadeDetail", "facadeDetail", null);
  refreshFacadeRows();

  // An older build kept a facade export folder here. The field is gone, the
  // preview reads the cache and generation fetches into it, so the leftover
  // key means nothing and is dropped rather than left to look meaningful.
  localStorage.removeItem('facadeDir');

  const facadeModeGroup = document.getElementById("facade-mode-group");
  facadeModeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      localStorage.setItem('facadeMode', btn.dataset.facadeMode);
      refreshFacadeRows();
    });
  });
  refreshFacadeRows();

  function applyCustomTileUrl() {
    const raw = customTileInput.value.trim();

    // An empty field is the normal way to go back to the themes. A non-empty
    // one that is not a usable template is left in the box so the user can see
    // and correct it, but is not handed to the map.
    if (raw && !isValidTileTemplate(raw)) {
      window.arnisLog('warn', 'Ignoring custom map source: expected an http(s) URL containing {z}, {x} and {y}.');
      localStorage.removeItem('customTileUrl');
    } else if (raw) {
      localStorage.setItem('customTileUrl', raw);
    } else {
      localStorage.removeItem('customTileUrl');
    }

    const mapIframe = getMapFrame();
    if (mapIframe && mapIframe.contentWindow) {
      mapIframe.contentWindow.postMessage({
        type: 'setCustomTileUrl',
        url: getCustomTileUrl()
      }, '*');
    }
  }

  // On change, not on input: a half-typed URL is not a source, and remounting
  // the basemap per keystroke would hammer whatever host they are aiming at.
  customTileInput.addEventListener("change", applyCustomTileUrl);
  refreshCustomSourceRow();

  // Telemetry consent toggle
  const telemetryToggle = document.getElementById("telemetry-toggle");
  const telemetryKey = 'telemetry-consent';

  // Load saved telemetry consent
  const savedConsent = localStorage.getItem(telemetryKey);
  telemetryToggle.checked = savedConsent === 'true';

  // Handle telemetry consent change
  telemetryToggle.addEventListener("change", () => {
    const isEnabled = telemetryToggle.checked;
    localStorage.setItem(telemetryKey, isEnabled ? 'true' : 'false');
    syncTelemetryConsent();
  });


  /// License and Credits
  async function openLicense() {
    const licenseModal = document.getElementById("license-modal");
    const licenseContent = document.getElementById("license-content");

    licenseContent.innerHTML = licenseText;
    licenseContent.scrollTop = 0;
    showModal(licenseModal);

    // The credits only known at runtime go into their own spot in the
    // "Models, Textures and Fonts" list. Held by reference, so a slow answer
    // from an earlier opening lands in that opening's detached copy rather
    // than doubling up in this one.
    const runtime = licenseContent.querySelector("#license-runtime-credits") || licenseContent;
    const credit = (title, body) =>
      `<div class="credit"><h4>${title}</h4>${body}</div>`;

    runtime.insertAdjacentHTML("beforeend", credit(
      "3D Model Repository (3DMR)",
      `<p>Landmark models from <a href="https://3dmr.eu" target="_blank" rel="noopener noreferrer">3dmr.eu</a> are fetched on demand and voxelized. Individual models retain the license declared by their uploader; specific per-model attribution is printed to the generation log. See the <a href="https://3dmr.eu" target="_blank" rel="noopener noreferrer">3DMR website</a> for any model used.</p>`
    ));

    // The premade facade set. All CC0, so attribution is a courtesy rather than
    // a condition, but the sources are named because someone should be able to
    // find them and because it says plainly that the pixels are free to ship.
    runtime.insertAdjacentHTML("beforeend", credit(
      "Preset Building Facade Textures",
      `<p>The photographs hung on buildings by the Preset Facades setting, all released under ` +
      `<a href="https://creativecommons.org/publicdomain/zero/1.0/" target="_blank" rel="noopener noreferrer">CC0</a>:</p>` +
      `<ul>` +
      `<li>Urban building, apartment and shop front photographs by <b>Scouser</b>, from ` +
      `<a href="https://opengameart.org/content/free-urban-textures-buildings-apartments-shop-fronts" target="_blank" rel="noopener noreferrer">OpenGameArt</a></li>` +
      `<li>Tiling facade materials from <b>TextureCan</b>: ` +
      `<a href="https://www.texturecan.com/details/315/" target="_blank" rel="noopener noreferrer">315</a>, ` +
      `<a href="https://www.texturecan.com/details/316/" target="_blank" rel="noopener noreferrer">316</a>, ` +
      `<a href="https://www.texturecan.com/details/357/" target="_blank" rel="noopener noreferrer">357</a>, ` +
      `<a href="https://www.texturecan.com/details/360/" target="_blank" rel="noopener noreferrer">360</a>, ` +
      `<a href="https://www.texturecan.com/details/563/" target="_blank" rel="noopener noreferrer">563</a></li>` +
      `</ul>`
    ));

    const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));
    const link = (url, text) =>
      `<a href="${esc(url)}" target="_blank" rel="noopener noreferrer">${esc(text)}</a>`;

    // Mapillary imagery is CC BY-SA, and the licence is on the pixels: a world
    // built from street photographs has to name the photographers. The source
    // is named whether or not a run has used it yet, so the credit can be
    // found before the first generation; the per-image list underneath, one
    // line per photograph in the shape Mapillary's own guidance asks for,
    // fills in after one.
    let shots = [];
    try {
      const used = await invoke("gui_get_mapillary_attributions");
      if (Array.isArray(used)) shots = used;
    } catch (e) {
      console.warn("Failed to load Mapillary attributions:", e);
    }
    let mapillaryList;
    if (shots.length > 0) {
      // A record that carries the image id but not the uploader name has no
      // profile to link to, so the name is shown as plain text instead.
      const lines = shots.map((s) => {
        const by = s.profile_url ? link(s.profile_url, s.username) : esc(s.username);
        return `<li>${link(s.image_url, s.title)} by ${by}, licensed under CC-BY-SA</li>`;
      }).join("");
      const unnamed = shots.filter((s) => !s.profile_url).length;
      const note = unnamed > 0
        ? ` ${unnamed} of these name only the photograph: their records carry the image id but not the uploader name. Each link opens the image, which names its uploader.`
        : "";
      mapillaryList =
        `<p>Photographs used by the last generation:${note}</p>` +
        `<ul>${lines}</ul>`;
    } else {
      mapillaryList =
        `<p>The photographs a generation used are listed here once it has run.</p>`;
    }
    runtime.insertAdjacentHTML("beforeend", credit(
      "Building Facades (Mapillary)",
      `<p>The Mapillary facade source measures wall textures and colours from street-level photographs on ` +
      `${link("https://www.mapillary.com", "Mapillary")}, licensed ` +
      `${link("https://creativecommons.org/licenses/by-sa/4.0/", "CC BY-SA 4.0")}. ` +
      `Share-alike applies to anything you publish that carries them.</p>` +
      mapillaryList
    ));

    try {
      const rows = await invoke("gui_get_3d_model_attributions");
      if (Array.isArray(rows) && rows.length > 0) {
        const items = rows.map(r => {
          const lic = r.license_url
            ? `<a href="${esc(r.license_url)}" target="_blank" rel="noopener noreferrer">${esc(r.license)}</a>`
            : esc(r.license);
          return `<li><b>${esc(r.label)}</b> — ${esc(r.artist)}, ${lic} (<a href="${esc(r.source_url)}" target="_blank" rel="noopener noreferrer">source</a>)</li>`;
        }).join("");
        runtime.insertAdjacentHTML("beforeend", credit(
          "Bundled 3D Models (Wikimedia Commons via Wikidata P4896)",
          `<p>Permissive-licensed models used to render famous landmarks. Voxelized and rescaled by Arnis.</p>` +
          `<ul>${items}</ul>`
        ));
      }
    } catch (e) {
      console.warn("Failed to load 3D model attributions:", e);
    }
  }

  function closeLicense() {
    const licenseModal = document.getElementById("license-modal");
    hideModal(licenseModal);
  }

  window.openLicense = openLicense;
  window.closeLicense = closeLicense;
}

// World format selection (Java/Bedrock/Luanti)
let selectedWorldFormat = 'java'; // Default to Java

const VALID_FORMATS = ['java', 'bedrock', 'luanti'];

// Voxy renders distant terrain from the per-voxel light stored in its LOD
// cache, so pre-generating one without baked lighting would give a black
// horizon. Keep the two toggles consistent in the UI rather than quietly
// overriding the user's choice at generation time.
function initVoxyLightingCoupling() {
  const voxy = document.getElementById('voxy-lod-toggle');
  const bake = document.getElementById('bake-lighting-toggle');
  if (!voxy || !bake) return;

  // Dispatch so the settings store persists the knock-on change too.
  const set = (el, value) => {
    if (el.checked === value) return;
    el.checked = value;
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };

  voxy.addEventListener('change', () => {
    if (voxy.checked) set(bake, true);
  });
  bake.addEventListener('change', () => {
    if (!bake.checked) set(voxy, false);
  });
}

// The Extra Features switch (ids still say advanced-features). Off, its groups are hidden and every control
// in them disabled: a hidden value then raises no revert arrow or nav dot, and
// startGeneration sends nothing, so Arnis runs exactly as stock.
function initAdvancedFeatures() {
  const master = document.getElementById('advanced-features-toggle');
  const groups = document.getElementById('advanced-features-groups');
  const cpu = document.getElementById('cpu-usage-slider');
  if (!master || !groups || !cpu) return;
  // The same switch at the end of Settings sets this one; refreshAdvancedFeatures follows back.
  const mirror = document.getElementById('advanced-features-end-toggle');
  mirror.addEventListener('change', () => {
    if (master.checked === mirror.checked) return;
    master.checked = mirror.checked;
    master.dispatchEvent(new Event('change', { bubbles: true }));
  });
  // Big Worlds comes on with the switch. The store restores in DOM order, so a
  // stored Big Worlds off is written after this and wins.
  master.addEventListener('change', () => {
    const big = document.getElementById('big-worlds-toggle');
    if (master.checked && big && !big.checked) {
      big.checked = true;
      big.dispatchEvent(new Event('change', { bubbles: true }));
    }
    refreshAdvancedFeatures();
  });
  document.getElementById('big-worlds-toggle').addEventListener('change', refreshAdvancedFeatures);
  cpu.addEventListener('input', () => { formatCpuUsage(); refreshAdvancedFeatures(); });
  cpu.addEventListener('dblclick', () => {
    cpu.value = 0;
    fireInputChange(cpu);
  });
  document.getElementById('threads-input').addEventListener('input', refreshAdvancedFeatures);
  // Meld Generation rows that follow another control.
  ['snow-mode-select', 'rocks-toggle', 'bushes-toggle', 'field-mix-select', 'farm-crops-input',
    'interior-toggle', 'caves-toggle', 'region-format-select', 'grass-texture-toggle',
    'land-texture-toggle', 'disable-height-limit-toggle', 'props-select', 'osm-source-select',
    'offline-toggle'].forEach((id) => {
    document.getElementById(id).addEventListener('input', refreshAdvancedFeatures);
    document.getElementById(id).addEventListener('change', refreshAdvancedFeatures);
  });
  MELD_SLIDERS.forEach(([id, format]) => {
    const slider = document.getElementById(id);
    const out = document.getElementById(id.replace('-slider', '-value'));
    const show = () => { out.textContent = format(parseFloat(slider.value)); };
    slider.addEventListener('input', show);
    // Double-click resets, as on the other sliders.
    slider.addEventListener('dblclick', () => {
      slider.value = slider.defaultValue;
      fireInputChange(slider);
    });
    show();
  });
  const loot = document.getElementById('loot-table-input');
  bindBrowse('loot-table-browse', loot, 'gui_pick_loot_table', () => loot.value.trim());
  initPropFamilies();
  initExperimentalButtons();
  initTreePack();
  // The cards follow their controls, restored and reset values included.
  initTreeSizeToggles();
  initMixRows();
  initPreviewModes();
  groups.addEventListener('change', refreshOptionPreviews);
  // Offline, a live render may only read the caches.
  document.getElementById('offline-toggle').addEventListener('change', refreshLivePreviews);
  refreshOptionPreviews();
  initOsmSource();
  initPresets();
  // The workers and memory rows size the "W workers" of the size line.
  ['unit-regions-select', 'snap-mode-select', 'square-selection-toggle', 'scale-value-slider',
    'one-world-workers-select', 'ram-budget-input', 'max-downloads-input'].forEach((id) => {
    document.getElementById(id).addEventListener('change', refreshSnapPreview);
  });
  formatCpuUsage();
  refreshAdvancedFeatures();
}

// OSM Data Source: the local file picker and the offline download, which
// runs the generation's own settings with --prewarm.
function initOsmSource() {
  const file = document.getElementById('osm-file-input');
  bindBrowse('osm-file-browse', file, 'gui_pick_osm_file', () => file.value.trim());
  const pbf = document.getElementById('osm-pbf-input');
  bindBrowse('osm-pbf-browse', pbf, 'gui_pick_pbf_file', () => pbf.value.trim());
  // The bake is the OSM step of a prewarm: same settings, same threads, same
  // progress, and the panel's bar and Stop follow it.
  document.getElementById('offline-download-button').addEventListener('click', () => downloadAndBakeMissing());
  initOfflineFirst();
  ['prewarm-button', 'osm-pbf-bake-button', 'data-plan-button'].forEach((id) =>
    document.getElementById(id).addEventListener('click', startDownload));
  document.getElementById('transfer-stop-button').addEventListener('click', () => {
    if (!transferJob) return;
    document.getElementById('transfer-stop-button').disabled = true;
    document.getElementById('transfer-stage').textContent = oneWorldText('transfer_stopping', 'Stopping...');
    invoke('gui_cancel_bake').catch((error) => console.warn('Stop failed:', error));
  });
  document.getElementById('storage-refresh-button').addEventListener('click', refreshStorage);
  const bakeCpuSlider = document.getElementById('bake-cpu-slider');
  bakeCpuSlider.addEventListener('input', formatBakeCpu);
  formatBakeCpu();
  initLocalArchive();
  // Any setting can change what a run reads; the check is debounced and skips
  // a request it has already answered.
  document.getElementById('settings-modal').addEventListener('change', () => refreshDataPlan());
  refreshStorage();
}

/* Download Plan: what a run of the selection reads with these settings, and
   how much of it the caches hold, from gui_data_plan (disk only). Shown where
   a run depends on the caches: offline, or reading a local extract or file. */
let dataPlanTimer = null;
let dataPlanKey = null;
let dataPlan = null;
// Each source's cached file count when the running download started.
let dataPlanStart = null;
// The extract sizes already asked for with a HEAD, by url.
const extractSizeAsked = new Set();

// `force` asks again even for the same request: the caches changed under it.
function refreshDataPlan(force) {
  if (force) dataPlanKey = null;
  clearTimeout(dataPlanTimer);
  dataPlanTimer = setTimeout(checkDataPlan, 300);
}

function dataPlanRequest() {
  const source = document.getElementById('osm-source-select').value;
  const offline = document.getElementById('offline-toggle').checked;
  // Extra Features shows it on the main window for any source.
  if (!selectedBBox || !(extraFeaturesOn() || offline || source === 'pbf' || source === 'file' || source === 'local')) return null;
  const mode = document.getElementById('generation-mode-select').value;
  return {
    bboxText: selectedBBox,
    worldScale: parseFloat(document.getElementById('scale-value-slider').value) || 1,
    terrainEnabled: mode === 'geo-terrain' || mode === 'terrain-only',
    skipOsmObjects: mode === 'terrain-only',
    canopyHeightEnabled: document.getElementById('canopy-height-toggle').checked,
    overtureEnabled: document.getElementById('overture-toggle').checked,
    awsOnlyElevation: document.getElementById('aws-only-elevation-toggle').checked,
    flags: advancedFeatureArgs().flags,
  };
}

async function checkDataPlan() {
  const request = dataPlanRequest();
  document.getElementById('data-plan-row').style.display = request ? '' : 'none';
  checkPreparePlan();
  checkBakeThreads();
  const key = request ? JSON.stringify(request) : null;
  if (!request || key === dataPlanKey) return;
  dataPlanKey = key;
  try {
    const plan = await invoke('gui_data_plan', request);
    // A newer request went out while this one was on disk.
    if (key !== dataPlanKey) return;
    dataPlan = plan;
    renderDataPlan();
    askExtractSize(plan.extract);
  } catch (error) {
    console.warn('Download plan failed:', error);
  }
}

// A Region Download extract not on disk yet: its size from one HEAD request,
// kept by the backend, then the plan again. Never offline.
async function askExtractSize(extract) {
  if (!extract || !extract.url || extract.download_bytes != null) return;
  if (document.getElementById('offline-toggle').checked || extractSizeAsked.has(extract.url)) return;
  extractSizeAsked.add(extract.url);
  try {
    await invoke('gui_extract_size', { url: extract.url });
    refreshDataPlan(true);
  } catch (error) {
    console.warn('Extract size failed:', error);
  }
}

function formatPlanBytes(bytes) {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(1) + ' GB';
  if (bytes >= 1e6) return Math.round(bytes / 1e6) + ' MB';
  return Math.max(1, Math.round(bytes / 1e3)) + ' KB';
}

// A size cell: nothing for zero, `~` for an estimate.
function sizeCell(bytes, estimate) {
  if (!bytes) return '—';
  return (estimate ? '~' : '') + formatPlanBytes(bytes);
}

// A plan list row: name, status (green when `done`) and the size columns;
// with `percent`, the transfer bar under it while its item comes in.
function planRow(cells, done, percent) {
  const li = document.createElement('li');
  cells.forEach((text, i) => {
    const span = document.createElement('span');
    span.textContent = text;
    if (i === 1 && done) span.className = 'is-cached';
    if (i >= 2) span.className = 'data-plan-size';
    li.appendChild(span);
  });
  if (percent != null) {
    const bar = document.createElement('div');
    bar.className = 'progress-bar-container data-plan-bar';
    bar.innerHTML = '<div class="progress-bar"></div>';
    bar.firstChild.style.width = Math.max(0, Math.min(100, percent)) + '%';
    li.appendChild(bar);
  }
  return li;
}

function planHead(cells) {
  const li = planRow(cells, false);
  li.className = 'data-plan-head';
  return li;
}

// A line under a list; `warn` turns it red.
function setPlanLine(id, text, warn) {
  const el = document.getElementById(id);
  el.textContent = text || '';
  el.style.display = text ? '' : 'none';
  el.classList.toggle('is-warn', !!warn);
}

function renderDataPlan() {
  const list = document.getElementById('data-plan-list');
  if (!list || !dataPlan) return;
  const t = oneWorldText;
  const names = {
    osm: t('data_plan_osm', 'OpenStreetMap'),
    elevation: t('data_plan_elevation', 'Elevation'),
    land_cover: t('data_plan_land_cover', 'Land Cover'),
    canopy: t('data_plan_canopy', 'Canopy Height'),
    overture: t('data_plan_overture', 'Overture Buildings'),
  };
  let download = 0;
  list.classList.add('has-sizes');
  list.replaceChildren(planHead([t('data_plan_col_source', 'Source'), t('data_plan_col_status', 'Status'),
    t('data_plan_col_on_disk', 'On Disk'), t('data_plan_col_download', 'To Download')]),
  ...dataPlan.items.map((item) => {
    let status;
    if (item.total === 0) {
      status = t('data_plan_uncounted', 'Not counted');
    } else if (item.cached === item.total) {
      status = t('data_plan_cached', 'Cached ✓');
    } else if (item.cached > 0) {
      status = t('data_plan_partly', 'Partly cached ({n}/{m})', { n: item.cached, m: item.total });
    } else {
      status = t('data_plan_missing', 'Missing');
    }
    const missing = item.cached < item.total && item.missing_bytes ? item.missing_bytes : 0;
    download += missing;
    // While a download runs the plan is read again every two seconds, so a
    // row's bar is its files on disk out of the files it needs.
    const running = transferJob === 'prewarm' && dataPlanStart && item.total > 0 &&
      dataPlanStart[item.source] < item.total;
    return planRow([names[item.source] || item.source, status, sizeCell(item.cached_bytes),
      sizeCell(missing, true)], item.total > 0 && item.cached === item.total,
    running ? 100 * item.cached / item.total : null);
  }));
  const e = dataPlan.extract;
  let line = '';
  let bake = 0;
  if (e && e.name) {
    const parts = [];
    if (e.bytes != null) parts.push(t('data_plan_extract_on_disk', '{size} on disk', { size: formatPlanBytes(e.bytes) }));
    else if (e.download_bytes != null) parts.push(t('data_plan_extract_download', '{size} to download', { size: formatPlanBytes(e.download_bytes) }));
    if (e.bake_bytes != null) {
      parts.push(t('data_plan_bake_on_disk', 'baked: {size}', { size: formatPlanBytes(e.bake_bytes) }));
    } else {
      if (e.bake_estimate) {
        bake = e.bake_estimate;
        parts.push(t('data_plan_bake_estimate', 'bake ~{size} on disk', { size: formatPlanBytes(e.bake_estimate) }));
      }
      if (e.ram_estimate) parts.push(t('data_plan_bake_ram', '~{size} memory while baking', { size: formatPlanBytes(e.ram_estimate) }));
    }
    line = parts.length
      ? [t('data_plan_extract_name', 'Extract: {name}', { name: e.name })].concat(parts).join(' · ')
      : t('data_plan_extract_unknown', 'Extract: {name}, size known once downloaded', { name: e.name });
  } else if (e) {
    line = t('data_plan_no_index', 'The Geofabrik index is not downloaded yet; the button fetches it.');
  }
  const local = dataPlan.local_archive;
  if (local) {
    const here = local.archives.filter((a) => a.covers)
      .map((a) => (a.bytes ? a.name + ' (' + formatPlanBytes(a.bytes) + ')' : a.name));
    line = here.length
      ? t('data_plan_local', 'In the folder for this area: {names}', { names: here.join(', ') })
      : t('data_plan_local_none', 'No archive in the folder covers this area yet.');
  }
  setPlanLine('data-plan-extract', line);
  // What the button would add to the disk, against what the disk has left.
  const need = download + bake;
  const free = dataPlan.free_bytes;
  let total = '';
  let short = false;
  if (free != null) {
    short = need > free;
    const vars = { download: formatPlanBytes(download), need: formatPlanBytes(need), free: formatPlanBytes(free) };
    if (short) total = t('data_plan_short', 'Not enough room: ~{need} needed, {free} free on this disk.', vars);
    else if (need > 0) total = t('data_plan_total', 'Total: ~{download} to download, ~{need} more on disk · {free} free', vars);
    else total = t('data_plan_total_none', 'Nothing to download · {free} free on this disk', vars);
  }
  setPlanLine('data-plan-total', total, short);
  const missingItems = dataPlan.items.filter((i) => i.total > 0 && i.cached < i.total).length;
  document.getElementById('offline-details-count').textContent = missingItems
    ? t('data_plan_missing', 'Missing') + ' ' + missingItems + '/' + dataPlan.items.length
    : t('data_plan_cached', 'Cached ✓');
  renderOfflineSummary();
}

/* Bake CPU: downloads and bakes run on their own share of the cores (default
   75 %), passed as --cpu-target in place of the generation's CPU setting, so
   the backend sizes them with the same model. */
function bakeCpu() {
  return parseInt(document.getElementById('bake-cpu-slider').value, 10) || 75;
}

function formatBakeCpu() {
  document.getElementById('bake-cpu-value').textContent = bakeCpu() + '%';
}

// The Extra Features flags with the bake's CPU share for the generation's.
function bakeFlags(flags) {
  return flags.filter((f) => !/^--(threads|cpu-target)(=|$)/.test(f)).concat('--cpu-target=' + bakeCpu());
}

/* Bake threads: what a bake runs on with the Bake CPU Usage. */
let bakeThreadsKey = null;

async function checkBakeThreads() {
  const source = document.getElementById('osm-source-select').value;
  const line = document.getElementById('bake-threads');
  if (source !== 'pbf' && source !== 'local') {
    line.style.display = 'none';
    return;
  }
  const flags = bakeFlags(advancedFeatureArgs().flags);
  const key = JSON.stringify(flags);
  if (key === bakeThreadsKey && line.textContent) {
    line.style.display = '';
    return;
  }
  bakeThreadsKey = key;
  try {
    const b = await invoke('gui_bake_threads', { flags });
    if (key !== bakeThreadsKey) return;
    setPlanLine('bake-threads', oneWorldText('bake_threads',
      'Bakes use {threads} of {cores} threads ({pct}% CPU), as the generation workers do; at most {downloads} downloads at once.',
      { threads: b.threads, cores: b.cores, pct: b.cpu_pct, downloads: b.downloads }));
  } catch (error) {
    console.warn('Bake threads failed:', error);
  }
}

/* The transfer panel: the bar and Stop of a running download or bake, fed by
   the `transfer` field of progress-update. `transferJob` is 'prewarm' or
   'bake' while one runs from this window. */
let transferJob = null;
let transferLast = null;
// While a download runs: the plan read again, for the rows' bars.
let transferPoll = null;

function transferStart(job) {
  transferJob = job;
  transferLast = null;
  const stop = document.getElementById('transfer-stop-button');
  stop.disabled = false;
  stop.style.display = '';
  document.getElementById('offline-download-button').hidden = true;
  const panel = document.getElementById('transfer-panel');
  panel.style.display = '';
  panel.classList.remove('is-ended', 'is-done');
  // The rows' bars: the plan, read off the disk again while it runs.
  dataPlanStart = dataPlan ? Object.fromEntries(dataPlan.items.map((i) => [i.source, i.cached])) : null;
  clearInterval(transferPoll);
  transferPoll = setInterval(() => refreshDataPlan(true), 2000);
  document.getElementById('transfer-stage').textContent = oneWorldText('transfer_starting', 'Starting...');
  document.getElementById('transfer-detail').textContent = '';
  setTransferBar(0);
}

// The run ended with `message`, `done` when it finished: the panel keeps the
// last bar with the outcome in place of the stage.
function transferEnd(message, done) {
  if (!transferJob) return;
  transferJob = null;
  clearInterval(transferPoll);
  dataPlanStart = null;
  // The last download's rate times the next one.
  if (transferLast && transferLast.stage === 'download' && transferLast.rate_bps > 0) offlineRate = transferLast.rate_bps;
  document.getElementById('transfer-stop-button').style.display = 'none';
  if (message) document.getElementById('transfer-stage').textContent = message;
  if (done) setTransferBar(100);
  document.getElementById('transfer-panel').classList.add('is-ended');
  document.getElementById('transfer-panel').classList.toggle('is-done', !!done);
}

function setTransferBar(percent) {
  const p = Math.max(0, Math.min(100, percent));
  document.getElementById('transfer-bar').style.width = p + '%';
  document.getElementById('transfer-percent').textContent = Math.floor(p) + '%';
}

// A rate: one decimal under 10 MB/s, where whole megabytes say too little.
function formatRate(bps) {
  return (bps >= 1e6 && bps < 1e7 ? (bps / 1e6).toFixed(1) + ' MB' : formatPlanBytes(bps)) + '/s';
}

function formatClock(seconds) {
  const s = Math.max(0, Math.round(seconds));
  return Math.floor(s / 60) + ':' + String(s % 60).padStart(2, '0');
}

// One progress-update while a job runs: `transfer` when the backend sent one.
function onTransferProgress(progress, message, transfer) {
  if (!transferJob) return;
  const t = oneWorldText;
  if (!transfer) {
    // Before the first transfer (or between them), the bar follows the run.
    if (!transferLast && progress >= 0) setTransferBar(progress);
    if (!transferLast && message && !message.startsWith('Done!') && !message.startsWith('Error!')) {
      document.getElementById('transfer-stage').textContent = message;
    }
    return;
  }
  transferLast = transfer;
  const vars = { name: transfer.name, n: transfer.item, m: transfer.items };
  const stage = {
    download: t('transfer_download', 'Downloading {name} ({n}/{m})', vars),
    bake: t('transfer_bake', 'Baking {name} ({n}/{m})', vars),
    finalize: t('transfer_finalize', 'Writing {name} ({n}/{m})', vars),
  }[transfer.stage];
  // No extract yet (arnis-tiles choosing them): its status line says more.
  if (!document.getElementById('transfer-stop-button').disabled) {
    document.getElementById('transfer-stage').textContent = transfer.name ? stage : message || stage;
  }
  setTransferBar(transfer.percent);
  const parts = [];
  const done = formatPlanBytes(transfer.done_bytes);
  if (transfer.total_bytes > 0) {
    parts.push(t('transfer_bytes', '{done} of {total}', { done, total: formatPlanBytes(transfer.total_bytes) }));
  } else if (transfer.done_bytes > 0) {
    parts.push(t('transfer_written', '{done} written', { done }));
  }
  if (transfer.stage === 'download' && transfer.rate_bps > 0) {
    parts.push(formatRate(transfer.rate_bps));
    if (transfer.total_bytes > transfer.done_bytes) {
      parts.push(t('transfer_left', 'about {time} left',
        { time: formatClock((transfer.total_bytes - transfer.done_bytes) / transfer.rate_bps) }));
    }
  }
  if (transfer.stage !== 'download' && transfer.threads > 0) {
    parts.push(t('transfer_threads', 'Baking with {threads} threads ({pct}% CPU)',
      { threads: transfer.threads, pct: transfer.cpu_pct }));
  }
  document.getElementById('transfer-detail').textContent = parts.join(' · ');
  if (transferJob === 'bake') renderPreparePlan();
}

// The panel's download buttons: a prewarm with the panel's bar and Stop.
async function startDownload() {
  if (bakeRunning || generationButtonEnabled === false) return;
  transferStart('prewarm');
  await startGeneration({ prewarm: true });
  // Refused before it started (no selection, say): nothing to follow.
  if (generationButtonEnabled !== false) {
    transferEnd('', false);
    if (!extraFeaturesOn()) document.getElementById('transfer-panel').style.display = 'none';
    renderOfflinePanel();
  }
}

/* Storage: each folder downloads and bakes go to, its size and the free space
   on its disk, from gui_storage_info. Only the Archive Folder can move. */
async function refreshStorage() {
  try {
    const folder = document.getElementById('local-archive-input').value.trim();
    renderStorage(await invoke('gui_storage_info', { folder }));
  } catch (error) {
    console.warn('Storage info failed:', error);
  }
}

function storageButton(icon, title, onClick) {
  const b = document.createElement('button');
  b.type = 'button';
  b.className = 'save-path-browse';
  b.title = title;
  b.setAttribute('aria-label', title);
  b.innerHTML = '<svg class="icon" aria-hidden="true"><use href="#i-' + icon + '"></use></svg>';
  b.addEventListener('click', onClick);
  return b;
}

function renderStorage(locations) {
  const t = oneWorldText;
  const names = {
    local_archive: t('storage_local_archive', 'Archive Folder (Local Archive)'),
    bake_scratch: t('storage_bake_scratch', 'Bake Work Folder (arnis-tiles)'),
    pbf_downloads: t('storage_pbf_downloads', 'Region Download Extracts'),
    pbf_bakes: t('storage_pbf_bakes', 'Region Download Bakes'),
    cache_root: t('storage_cache_root', 'Cache Folder (All Arnis Caches)'),
  };
  document.getElementById('storage-list').replaceChildren(...(locations || []).map((l) => {
    const li = document.createElement('li');
    li.className = 'storage-item';
    const head = document.createElement('div');
    head.className = 'storage-head';
    const name = document.createElement('span');
    name.textContent = names[l.kind] || l.kind;
    const size = document.createElement('span');
    size.className = 'storage-size';
    const free = l.free_bytes != null ? formatPlanBytes(l.free_bytes) : '?';
    size.textContent = l.exists && l.bytes > 0
      ? t('storage_size', '{used} · {free} free', { used: formatPlanBytes(l.bytes), free })
      : t('storage_empty', 'Empty · {free} free', { free });
    head.append(name, size);
    const control = document.createElement('div');
    control.className = 'save-path-control storage-path';
    const path = document.createElement('input');
    path.type = 'text';
    path.className = 'save-path-input';
    path.readOnly = true;
    path.value = l.path;
    path.title = l.path;
    path.spellcheck = false;
    const open = storageButton('external', 'Open folder', () =>
      invoke('gui_show_in_folder', { path: l.path }).catch((error) => console.warn('Open failed:', error)));
    open.disabled = !l.exists;
    control.append(path, open);
    if (l.changeable) {
      const field = document.getElementById('local-archive-input');
      control.append(storageButton('folder-open', 'Change...', async () => {
        try {
          const picked = await invoke('gui_pick_save_directory', { startPath: l.path });
          if (picked && picked !== l.path) {
            field.value = picked;
            fireInputChange(field);
          }
        } catch (error) {
          console.warn('Change failed:', error);
        }
      }));
    }
    li.append(head, control);
    return li;
  }));
}

/* Local Archive: a folder of countries baked by arnis-tiles, read through
   --osm-tiles-url. Prepare Countries lists the Geofabrik extracts that cover
   the selection (arnis-tiles prepare --dry-run, kept per bbox by the backend)
   and bakes them into the folder with Download & Bake. */
let localArchiveInfo = null;
let prepareKey = null;
let preparePlan = null;
let bakeRunning = false;
// How the last bake ended, shown once the list is back.
let bakeNote = '';
// The extracts the running bake has reached, in order.
let bakeSeen = [];

function localArchiveFolder() {
  const typed = document.getElementById('local-archive-input').value.trim();
  return typed || (localArchiveInfo && localArchiveInfo.default_folder) || null;
}

function arnisTilesPath() {
  return document.getElementById('arnis-tiles-path-input').value.trim();
}

async function refreshLocalArchiveInfo() {
  try {
    localArchiveInfo = await invoke('gui_local_archive_info', { tilesPath: arnisTilesPath() });
    if (localArchiveInfo) {
      document.getElementById('local-archive-input').placeholder = localArchiveInfo.default_folder;
    }
  } catch (error) {
    console.warn('Local archive info failed:', error);
  }
  prepareKey = null;
  refreshDataPlan(true);
}

function initLocalArchive() {
  bindBrowse('local-archive-browse', document.getElementById('local-archive-input'),
    'gui_pick_save_directory', () => localArchiveFolder() || '', 'startPath');
  document.getElementById('arnis-tiles-path-input').addEventListener('change', refreshLocalArchiveInfo);
  document.getElementById('local-archive-input').addEventListener('change', refreshStorage);
  document.getElementById('prepare-bake-button').addEventListener('click', bakeCountries);
  refreshLocalArchiveInfo();
}

function setPrepareStatus(text) {
  const status = document.getElementById('prepare-status');
  status.replaceChildren(text);
  status.style.display = text ? '' : 'none';
}

async function checkPreparePlan() {
  const panel = document.getElementById('prepare-panel');
  const source = document.getElementById('osm-source-select').value;
  const show = source === 'local' && !!selectedBBox;
  panel.style.display = show ? '' : 'none';
  if (!show || bakeRunning) return;
  if (!localArchiveInfo || !localArchiveInfo.arnis_tiles) {
    // Says how to get it rather than only greying the button.
    preparePlan = null;
    prepareKey = null;
    renderPreparePlan();
    const link = document.createElement('a');
    link.href = 'https://github.com/louis-e/arnis-tiles';
    link.textContent = 'github.com/louis-e/arnis-tiles';
    link.addEventListener('click', (e) => { e.preventDefault(); openExternal(link.href); });
    const status = document.getElementById('prepare-status');
    status.replaceChildren(oneWorldText('prepare_missing_tool',
      'Baking needs arnis-tiles next to Arnis, on PATH or in arnis-tiles Path. Get it from') + ' ', link, '.');
    status.style.display = '';
    return;
  }
  const request = { bboxText: selectedBBox, folder: localArchiveFolder() || '', tilesPath: arnisTilesPath() };
  const key = JSON.stringify(request);
  if (key === prepareKey) return;
  prepareKey = key;
  preparePlan = null;
  renderPreparePlan();
  setPrepareStatus(oneWorldText('prepare_loading', 'Finding the countries that cover the selection...'));
  try {
    const plan = await invoke('gui_prepare_plan', request);
    // A newer request went out while arnis-tiles was answering this one.
    if (key !== prepareKey) return;
    preparePlan = plan;
    setPrepareStatus(bakeNote);
    bakeNote = '';
  } catch (error) {
    if (key !== prepareKey) return;
    prepareKey = null;
    setPrepareStatus(String(error));
  }
  renderPreparePlan();
}

// A country's status: baked, or where the running bake is with it.
function prepareStatus(e) {
  const t = oneWorldText;
  if (e.baked) return t('prepare_baked', 'Baked ✓');
  if (!bakeRunning || !transferLast) return '';
  if (transferLast.name === e.id) {
    return {
      download: t('prepare_downloading', 'Downloading {pct}%', {
        pct: transferLast.total_bytes ? Math.floor(100 * transferLast.done_bytes / transferLast.total_bytes) : 0,
      }),
      bake: t('prepare_baking', 'Baking'),
      finalize: t('prepare_writing', 'Writing'),
    }[transferLast.stage];
  }
  return bakeSeen.includes(e.id) ? t('prepare_baked', 'Baked ✓') : t('prepare_waiting', 'Waiting');
}

function renderPreparePlan() {
  const t = oneWorldText;
  const rows = preparePlan ? preparePlan.extracts : [];
  if (bakeRunning && transferLast && !bakeSeen.includes(transferLast.name)) bakeSeen.push(transferLast.name);
  const list = document.getElementById('prepare-list');
  list.classList.add('has-sizes');
  list.replaceChildren(...(rows.length ? [planHead([t('prepare_col_country', 'Country'), t('data_plan_col_status', 'Status'),
    t('prepare_col_download', 'Download'), t('prepare_col_archive', 'Archive')])] : []),
  ...rows.map((e) => {
    const status = prepareStatus(e);
    const done = status === t('prepare_baked', 'Baked ✓');
    const baking = bakeRunning && transferLast && transferLast.name === e.id ? transferLast.percent : null;
    return planRow([e.name, status, formatPlanBytes(e.bytes), sizeCell(e.archive_bytes, !e.baked)], done, baking);
  }));
  if (preparePlan && !rows.length) {
    setPrepareStatus(t('prepare_none', 'No Geofabrik extract covers the selection.'));
  }
  // What a bake of the countries not baked yet takes, against the folder's disk.
  const todo = rows.filter((e) => !e.baked);
  let total = '';
  let short = false;
  if (preparePlan && todo.length && preparePlan.free_bytes != null) {
    const vars = {
      download: formatPlanBytes(todo.reduce((a, e) => a + e.bytes, 0)),
      archive: formatPlanBytes(todo.reduce((a, e) => a + e.archive_bytes, 0)),
      peak: formatPlanBytes(preparePlan.peak_bytes),
      free: formatPlanBytes(preparePlan.free_bytes),
    };
    short = preparePlan.peak_bytes > preparePlan.free_bytes;
    total = short
      ? t('prepare_short', 'Not enough room: up to ~{peak} needed while baking, {free} free on this disk.', vars)
      : t('prepare_total', 'To bake: {download} to download, ~{archive} of archives · up to ~{peak} on disk while baking · {free} free', vars);
  }
  setPlanLine('prepare-total', total, short);
  document.getElementById('prepare-bake-button').disabled =
    bakeRunning || !rows.length || rows.every((e) => e.baked);
}

async function bakeCountries() {
  if (bakeRunning || !selectedBBox || transferJob) return;
  bakeRunning = true;
  bakeSeen = [];
  setGenerationButtonEnabled(false);
  transferStart('bake');
  renderPreparePlan();
  bakeNote = '';
  // The transfer panel below shows the bake from here on.
  setPrepareStatus('');
  let outcome = '';
  let finished = false;
  try {
    const done = await invoke('gui_bake_archive', {
      bboxText: selectedBBox,
      folder: localArchiveFolder() || '',
      tilesPath: arnisTilesPath(),
      flags: bakeFlags(advancedFeatureArgs().flags),
    });
    if (!done) bakeNote = oneWorldText('prepare_stopped', 'Stopped. Countries already baked are kept.');
    finished = done;
    outcome = done ? oneWorldText('prepare_done', 'Done! The local archive is ready.') : bakeNote;
  } catch (error) {
    bakeNote = String(error);
    outcome = bakeNote;
  } finally {
    bakeRunning = false;
    transferEnd(outcome, finished);
    setGenerationButtonEnabled(true);
    prepareKey = null;
    refreshDataPlan(true);
    refreshStorage();
  }
  return finished;
}

/* "Download & bake now" on the main window, and Start with "Download & bake
   missing data first" on: the countries a Local Archive still has to bake,
   then a download of everything else the selection reads. */
async function downloadAndBakeMissing() {
  const local = document.getElementById('osm-source-select').value === 'local';
  if (local && preparePlan && preparePlan.extracts.some((x) => !x.baked) && !(await bakeCountries())) {
    generateAfterDownload = false;
    return;
  }
  await startDownload();
  if (!transferJob) generateAfterDownload = false;
}

// The switch beside Start: on, Start fetches what is missing first, in the
// panel's bar, then generates. A main-window preference, kept like the
// world format, not a setting of the Settings page.
const OFFLINE_FIRST_KEY = 'arnis-offline-first';
let generateAfterDownload = false;

function offlineFirstOn() {
  try {
    return localStorage.getItem(OFFLINE_FIRST_KEY) === '1';
  } catch (_) {
    return false;
  }
}

function syncOfflineFirst() {
  const button = document.getElementById('offline-first-toggle');
  const on = offlineFirstOn();
  button.setAttribute('aria-pressed', String(on));
  button.classList.toggle('is-on', on);
  button.setAttribute('aria-label', oneWorldText('offline_first', 'Download & bake missing data first'));
  button.title = on
    ? oneWorldText('offline_first_on', 'Download & bake missing data first: on. Start fetches what is missing, then generates.')
    : oneWorldText('offline_first_off', 'Download & bake missing data first: off. Start generates straight away.');
}

function initOfflineFirst() {
  document.getElementById('offline-first-toggle').addEventListener('click', () => {
    try {
      localStorage.setItem(OFFLINE_FIRST_KEY, offlineFirstOn() ? '0' : '1');
    } catch (_) { /* a private window keeps it off */ }
    syncOfflineFirst();
  });
  syncOfflineFirst();
}

// Presets: the Extra Features and OSM Data Source settings as a JSON file.
function initPresets() {
  // Every setting on the page (version 2). A version 1 file held only these
  // two sections, so it leaves the others as they are.
  const all = [document.getElementById('settings-modal')];
  const v1 = ['settings-section-features', 'settings-section-osm'].map((id) => document.getElementById(id));
  const flash = (button, ok) => {
    button.classList.add(ok ? 'is-success' : 'is-error');
    setTimeout(() => button.classList.remove('is-success', 'is-error'), 1500);
  };
  const save = document.getElementById('preset-save-button');
  save.addEventListener('click', async () => {
    try {
      const contents = JSON.stringify({ arnisPreset: 2, settings: exportSettings(all) }, null, 2);
      if (await invoke('gui_save_preset', { contents })) flash(save, true);
    } catch (error) {
      console.error('Saving the preset failed:', error);
      flash(save, false);
    }
  });
  const load = document.getElementById('preset-load-button');
  load.addEventListener('click', async () => {
    try {
      const text = await invoke('gui_load_preset');
      if (text === null || text === undefined) return;
      const preset = JSON.parse(text);
      if (!preset || ![1, 2].includes(preset.arnisPreset) || typeof preset.settings !== 'object') {
        throw new Error('not an Arnis preset');
      }
      importSettings(preset.settings, preset.arnisPreset === 1 ? v1 : all);
      refreshOptionPreviews();
      flash(load, true);
    } catch (error) {
      console.error('Loading the preset failed:', error);
      flash(load, false);
    }
  });
}

// Option preview cards. A shipped picture shows the option a control holds
// (rendered with Arnis itself); when the rest of
// its group differs from the stock defaults, a live render of the group's
// sample area with its current flags replaces it (gui_render_preview).
const PREVIEW_GROUPS = {
  fields: ['field-mix', 'farm-crops', 'field-scale'],
  trees: ['tree-realm', 'tree-size-weights', 'tree-pack-dir', 'tree-pack-mode'],
  snow: ['snow-mode', 'snow-percent', 'snow-y'],
  scatter: ['rocks', 'rock-density', 'bushes', 'bush-density'],
  roads: ['road-detail'],
  water: ['river-bed', 'water-detail'],
  grass: ['grass-texture', 'grass-mix'],
  land: ['land-texture', 'land-mix'],
  // The ores do not change the cave zone map.
  caves: ['cave-style', 'cave-biomes'],
};
const TREE_SIZES = ['small', 'medium', 'big', 'tall', 'giant'];

// The shipped picture (work/previews/final_render.py, option B: one per
// option) a frame's current settings match, as a file under images/previews/
// (the 3D one, if any, has the same name under iso/), or null when only a
// live 2D render shows them: any custom value. Rocks and Bushes are drawn at
// the densest setting, so the formations show; they stand for the default
// densities. Climate is 2D only.
// Grass and Land Texture: off, on with the default mix, or on with another
// preset; changed shares have no shipped picture.
function mixPicture(kind, fallback) {
  if (!document.getElementById(kind + '-texture-toggle').checked) return kind + '-texture-off.webp';
  const text = document.getElementById(kind + '-mix-input').value.trim();
  if (text.includes('=')) return null;
  const preset = text || fallback;
  // Classic lays no parcels: the same ground as the texture off.
  if (preset === 'classic') return kind + '-texture-off.webp';
  return preset === fallback ? kind + '-texture-on.webp' : kind + '-mix-' + preset + '.webp';
}

function blockPictureKey(card) {
  const el = (id) => document.getElementById(id);
  const stock = (id) => el(id).value === el(id).defaultValue;
  switch (card.dataset.for) {
    case 'tree-realm-select': {
      const realm = el('tree-realm-select').value;
      const w = TREE_SIZES.map((s) => parseFloat(el('tree-weight-' + s + '-slider').value));
      if (w.every((v) => v === 100)) return 'tree-realm-' + realm + '.webp';
      const single = w.filter((v) => v === 100).length === 1 && w.every((v) => v === 0 || v === 100);
      return realm === 'auto' && single ? 'tree-size-' + TREE_SIZES[w.indexOf(100)] + '.webp' : null;
    }
    case 'field-mix-select':
      return el('farm-crops-input').value.trim() === '' && stock('field-scale-slider')
        ? 'field-mix-' + el('field-mix-select').value + '.webp' : null;
    case 'road-detail-select':
      return 'road-detail-' + el('road-detail-select').value + '.webp';
    case 'rocks-toggle': {
      if (!stock('rock-density-slider') || !stock('bush-density-slider')) return null;
      const r = el('rocks-toggle').checked;
      const b = el('bushes-toggle').checked;
      return 'scatter-' + (r ? (b ? 'both' : 'rocks') : (b ? 'bushes' : 'off')) + '.webp';
    }
    case 'snow-mode-select': {
      const mode = el('snow-mode-select').value;
      return stock('snow-percent-slider') && (mode !== 'manual' || stock('snow-y-input'))
        ? 'snow-mode-' + mode + '.webp' : null;
    }
    case 'river-bed-select':
      return el('water-detail-select').value === 'default'
        ? 'river-bed-' + el('river-bed-select').value + '.webp' : null;
    case 'water-detail-select':
      return el('river-bed-select').value === 'off'
        ? 'water-detail-' + el('water-detail-select').value + '.webp' : null;
    case 'grass-texture-toggle':
      return mixPicture('grass', 'default');
    case 'land-texture-toggle':
      return mixPicture('land', 'patchwork');
    case 'climate-mode-select':
      return 'climate-mode-' + el('climate-mode-select').value + '.webp';
    case 'cave-style-select':
      return el('cave-biomes-input').value.trim() === ''
        ? 'cave-style-' + el('cave-style-select').value
          + (el('cave-ores-select').value === 'more' ? '-more-ores' : '') + '.webp'
        : null;
    default:
      return null;
  }
}
const livePreviews = {};

// The picture a card shows: 3D (images/previews/iso/) when picked and drawn
// for the shipped option, else the 2D one with a note. The pick is a viewer
// preference kept outside the settings store.
const PREVIEW_MODE_KEY = 'arnis-preview-mode';
let previewMode = '2d';
try { previewMode = localStorage.getItem(PREVIEW_MODE_KEY) === '3d' ? '3d' : '2d'; } catch (_) {}

function paintPreview(card) {
  const img = card.querySelector('img');
  const flat = card.dataset.shown || card.dataset.static;
  card.querySelectorAll('.preview-mode button').forEach((b) => {
    b.classList.toggle('active', b.dataset.mode === previewMode);
    b.setAttribute('aria-pressed', String(b.dataset.mode === previewMode));
  });
  // Frames without a 2D | 3D switch (Grass, Land, Climate) are 2D only.
  const want3d = previewMode === '3d' && !!card.querySelector('.preview-mode');
  // Live renders are 2D only.
  const iso = want3d && !card.dataset.live && card.dataset.exact === '1'
    ? card.dataset.static.replace('images/previews/', 'images/previews/iso/')
    : null;
  card.classList.toggle('is-coming', want3d && !iso);
  img.onerror = iso ? () => {
    img.onerror = null;
    card.classList.add('is-coming');
    img.setAttribute('src', flat);
  } : null;
  const src = iso || flat;
  if (img.getAttribute('src') !== src) img.setAttribute('src', src);
}

function initPreviewModes() {
  document.querySelectorAll('.preview-frame .preview-mode button').forEach((button) => {
    button.addEventListener('click', () => {
      previewMode = button.dataset.mode;
      try { localStorage.setItem(PREVIEW_MODE_KEY, previewMode); } catch (_) {}
      document.querySelectorAll('.preview-frame').forEach(paintPreview);
    });
  });
  document.querySelectorAll('.preview-frame').forEach(initPreviewZoom);
}

// Zoom inside a frame: wheel or pinch around the pointer, drag to pan,
// double-click to fit, +/- by steps. A CSS transform on the picture (drawn
// at 2-3x the frame), so nothing is rendered again; the picture always
// covers the frame. The view stays when the picture changes, for comparing.
const PREVIEW_MAX_ZOOM = 6;

function initPreviewZoom(card) {
  const img = card.querySelector('img');
  let k = 1;
  let x = 0;
  let y = 0;
  const apply = () => {
    const w = card.clientWidth;
    const h = card.clientHeight;
    k = Math.min(PREVIEW_MAX_ZOOM, Math.max(1, k));
    x = Math.min(0, Math.max(w * (1 - k), x));
    y = Math.min(0, Math.max(h * (1 - k), y));
    img.style.transform = k === 1 ? '' : 'translate(' + x + 'px, ' + y + 'px) scale(' + k + ')';
  };
  // Zoom by `f` keeping the frame point (px, py) still.
  const zoomAt = (f, px, py) => {
    const next = Math.min(PREVIEW_MAX_ZOOM, Math.max(1, k * f));
    x = px - (px - x) * (next / k);
    y = py - (py - y) * (next / k);
    k = next;
    apply();
  };
  const local = (e) => {
    const r = card.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };
  card.addEventListener('wheel', (e) => {
    e.preventDefault();
    zoomAt(Math.exp(-e.deltaY * 0.0015), ...local(e));
  }, { passive: false });
  card.addEventListener('dblclick', (e) => {
    if (e.target.closest('button')) return;
    k = 1;
    apply();
  });
  card.querySelectorAll('.preview-zoom button').forEach((b) => b.addEventListener('click', () => {
    zoomAt(b.dataset.zoom === '1' ? 1.5 : 1 / 1.5, card.clientWidth / 2, card.clientHeight / 2);
  }));
  // One pointer pans, two pinch.
  const pointers = new Map();
  let pinch = 0;
  img.addEventListener('pointerdown', (e) => {
    img.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, local(e));
    card.classList.add('is-panning');
  });
  img.addEventListener('pointermove', (e) => {
    if (!pointers.has(e.pointerId)) return;
    const [px, py] = local(e);
    const [ox, oy] = pointers.get(e.pointerId);
    pointers.set(e.pointerId, [px, py]);
    if (pointers.size === 1) {
      x += px - ox;
      y += py - oy;
      apply();
    } else if (pointers.size === 2) {
      const [a, b] = Array.from(pointers.values());
      const d = Math.hypot(a[0] - b[0], a[1] - b[1]);
      if (pinch) zoomAt(d / pinch, (a[0] + b[0]) / 2, (a[1] + b[1]) / 2);
      pinch = d;
    }
  });
  const up = (e) => {
    pointers.delete(e.pointerId);
    pinch = 0;
    if (!pointers.size) card.classList.remove('is-panning');
  };
  img.addEventListener('pointerup', up);
  img.addEventListener('pointercancel', up);
  new ResizeObserver(apply).observe(card);
}

// Tree Sizes: per size a switch and its weight, 0 to 100 % of the usual
// share. Off is 0 and greys the number; on from 0 puts it back at 100. The
// number stays enabled either way, since a disabled control sends nothing.
function initTreeSizeToggles() {
  const boxes = Array.from(document.querySelectorAll('.tree-size-row input[data-weight]'));
  const sync = () => boxes.forEach((box) => {
    const field = document.getElementById(box.dataset.weight);
    box.checked = parseFloat(field.value) > 0;
    box.disabled = field.disabled;
    box.closest('.tree-size-row').classList.toggle('is-off', !box.checked);
  });
  boxes.forEach((box) => {
    const field = document.getElementById(box.dataset.weight);
    box.addEventListener('change', () => {
      field.value = box.checked ? 100 : 0;
      fireInputChange(field);
    });
    field.addEventListener('input', sync);
    field.addEventListener('change', () => {
      const v = Math.round(parseFloat(field.value));
      const clamped = Number.isFinite(v) ? Math.min(100, Math.max(0, v)) : 100;
      if (String(clamped) !== field.value) {
        field.value = clamped;
        fireInputChange(field);
        return;
      }
      sync();
    });
    new MutationObserver(sync).observe(field, { attributes: true, attributeFilter: ['disabled'] });
  });
  sync();
}

// Farm Crops, Grass Mix and Land Mix: a row per part with a switch and its
// share, starting from the layout's or preset's own shares (the FieldMix
// presets in field_texture.rs). The hidden text field stays what the store
// keeps and generation sends: a share list only when the rows differ from
// the preset's, else the preset's name, or nothing for the default.
const CROP_KEYS = ['wheat', 'potato', 'carrot', 'beetroot', 'sunflower', 'pumpkin', 'fallow'];
const MIX_KEYS = ['coarse', 'plains', 'flower', 'farm', 'moss'];
const FIELD_PRESETS = {
  classic: { shares: [0, 0, 0, 0, 0], crops: [100, 0, 0, 0, 0, 0, 0] },
  smallholding: { shares: [12, 10, 4, 70, 4], crops: [22, 18, 18, 14, 12, 10, 6] },
  patchwork: { shares: [10, 8, 2, 75, 5], crops: [40, 15, 15, 8, 12, 5, 5] },
  prairie: { shares: [6, 6, 1, 85, 2], crops: [62, 8, 6, 4, 12, 2, 6] },
  pasture: { shares: [6, 58, 24, 6, 6], crops: [45, 10, 10, 5, 20, 5, 5] },
  // --grass-mix's own default (FieldMix::GRASS).
  default: { shares: [6, 64, 22, 0, 8] },
};
// Cave Biomes: each theme's percent of its default amount. A style is one
// amount for every theme (CaveStyle::amount in src/caves/mod.rs).
const CAVE_THEMES = ['lush', 'dripstone', 'deepdark', 'mushroom', 'ice', 'amethyst', 'volcanic', 'coral'];
const CAVE_STYLES = { vanilla: 0, 'more-vanilla': 18, 'all-mix': 100, 'more-mix': 180 };
// `layout`: the select whose value is the preset (else the Field Layout).
// `all`: every part is sent, 0 included, as --cave-biomes keeps the style's
// amount for a part left out; `max` is a part's top.
const MIXES = {
  'farm-crops-input': {
    keys: CROP_KEYS,
    original: (name) => FIELD_PRESETS[name].crops,
    encode: () => '',
  },
  'grass-mix-input': {
    keys: MIX_KEYS,
    fallback: 'default',
    original: (name) => FIELD_PRESETS[name].shares,
    encode: (name) => (name === 'default' ? '' : name),
  },
  'land-mix-input': {
    keys: MIX_KEYS,
    fallback: 'patchwork',
    original: (name) => FIELD_PRESETS[name].shares,
    encode: (name) => (name === 'patchwork' ? '' : name),
  },
  'cave-biomes-input': {
    keys: CAVE_THEMES,
    layout: 'cave-style-select',
    all: true,
    max: 200,
    original: (name) => CAVE_THEMES.map(() => CAVE_STYLES[name]),
    encode: () => '',
  },
};

function initMixRows() {
  Object.entries(MIXES).forEach(([id, mix]) => {
    const field = document.getElementById(id);
    const list = document.querySelector('.mix-list[data-mix="' + id + '"]');
    if (!field || !list) return;
    const select = document.querySelector('.mix-preset[data-mix="' + id + '"]');
    const layout = document.getElementById(mix.layout || 'field-mix-select');
    const max = mix.max || 100;
    const rows = mix.keys.map((k) => list.querySelector('.mix-row[data-key="' + k + '"]'));
    const reset = list.querySelector('.mix-reset');
    const presetName = () => (select ? select.value : layout.value);
    const original = () => mix.original(presetName());
    const same = (a, b) => a.every((v, i) => v === b[i]);
    const values = () => rows.map((r) => {
      const v = Math.round(parseFloat(r.querySelector('input[type="number"]').value));
      return r.querySelector('.switch').checked && Number.isFinite(v) ? Math.min(max, Math.max(0, v)) : 0;
    });
    const show = (vals) => {
      rows.forEach((r, i) => {
        r.querySelector('input[type="number"]').value = vals[i];
        r.querySelector('.switch').checked = vals[i] > 0;
        r.classList.toggle('is-off', vals[i] === 0);
      });
      if (reset) reset.disabled = same(vals, original());
    };
    const write = (text) => {
      if (field.value === text) return;
      field.value = text;
      fireInputChange(field);
    };
    // Rows to text: the preset as is, or the shares that are on.
    const commit = () => {
      const vals = values();
      show(vals);
      write(same(vals, original())
        ? mix.encode(presetName())
        : mix.keys.map((k, i) => (vals[i] > 0 || mix.all ? k + '=' + vals[i] : null)).filter(Boolean).join(','));
    };
    // Text to rows: restore, reset and preset files write the field.
    const read = () => {
      const text = field.value.trim().toLowerCase();
      if (select) {
        const named = text === '' ? mix.fallback : text;
        if (FIELD_PRESETS[named] && select.querySelector('option[value="' + named + '"]')) {
          select.value = named;
        }
      }
      let vals = original();
      if (text.includes('=')) {
        if (!mix.all) vals = mix.keys.map(() => 0);
        text.split(',').forEach((pair) => {
          const [k, v] = pair.split('=').map((t) => t.trim());
          const i = mix.keys.indexOf(k);
          if (i >= 0) vals[i] = Math.min(max, Math.max(0, parseInt(v, 10) || 0));
        });
      }
      show(vals);
    };
    rows.forEach((r, i) => {
      const box = r.querySelector('.switch');
      const num = r.querySelector('input[type="number"]');
      box.addEventListener('change', () => {
        if (box.checked && !(parseFloat(num.value) > 0)) num.value = original()[i] || (mix.all ? 100 : 10);
        // One part stays on: an all-zero list is refused (but no cave biome is Vanilla).
        if (!mix.all && !values().some((v) => v > 0)) box.checked = true;
        commit();
      });
      num.addEventListener('change', () => {
        box.checked = parseFloat(num.value) > 0;
        if (!mix.all && !values().some((v) => v > 0)) {
          num.value = original()[i] || 10;
          box.checked = true;
        }
        commit();
      });
    });
    const restart = () => { show(original()); commit(); };
    if (reset) reset.addEventListener('click', restart);
    // A new layout or preset starts again from its own shares.
    (select || layout).addEventListener('change', restart);
    field.addEventListener('change', read);
    const disable = () => {
      rows.forEach((r) => r.querySelectorAll('input').forEach((x) => { x.disabled = field.disabled; }));
      if (select) select.disabled = field.disabled;
    };
    new MutationObserver(disable).observe(field, { attributes: true, attributeFilter: ['disabled'] });
    disable();
    read();
  });
}

// The realm Auto would pick by the selection's centre, as tree_pack.rs
// realm_for_latlon does (its ecoregion pick usually agrees). First match wins.
const REALM_BOXES = [
  ['fl', 8, 31, -90, -60], ['ena', 8, 62, -100, -52], ['wna', 25, 72, -170, -100],
  ['sam', -56, 14, -82, -34], ['eur', 34, 72, -25, 40], ['afr', -36, 37, -19, 52],
  ['ind', -11, 29, 60, 155], ['asn', 5, 75, 40, 155], ['aus', -50, 0, 110, 180],
  ['aus', -50, 32, -180, -130],
];

function showAutoRealm() {
  const slot = document.querySelector('.realm-auto .realm-detected');
  if (!slot) return;
  const b = (selectedBBox || '').split(/[ ,]+/).map(parseFloat);
  let text = '';
  if (b.length === 4 && b.every(Number.isFinite) && b.some((v) => v !== 0)) {
    const lat = (b[0] + b[2]) / 2;
    const lon = (b[1] + b[3]) / 2;
    const hit = REALM_BOXES.find(([, a0, a1, o0, o1]) => lat >= a0 && lat <= a1 && lon >= o0 && lon <= o1);
    const code = hit ? hit[0] : 'vanilla-plus';
    const name = document.querySelector('.realm-grid .segment[data-value="' + code + '"] span[data-localize]');
    if (name) text = ' · ' + name.textContent.trim();
  }
  slot.textContent = text;
}

// What a card shows, in words. A side-by-side block names the picked option
// of each list and the switches that are on ("Europe · Small + Tall");
// all of a list's switches on is the stock mix and goes unsaid.
function previewName(card) {
  const block = card.closest('.preview-block');
  const picked = Array.from(block.querySelectorAll('.preview-options .segment.active')).map((b) => b.textContent.trim());
  const switches = Array.from(block.querySelectorAll('.tree-size-row input.switch, .preview-controls > .settings-row > .settings-control > input.switch'));
  const on = switches.filter((s) => s.checked).map((s) => {
    const label = s.closest('label') || s.closest('.settings-row').querySelector('.setting-heading > span');
    return label.textContent.trim();
  });
  if (switches.length && on.length < switches.length) {
    picked.push(on.length ? on.join(' + ') : oneWorldText('preview_off', 'Off'));
  } else if (switches.length && !picked.length) {
    picked.push(on.join(' + '));
  }
  // Rows changed from their preset's own.
  const mixed = Array.from(block.querySelectorAll('.mix-list[data-mix]'))
    .some((list) => document.getElementById(list.dataset.mix).value.includes('='));
  if (mixed) picked.push(oneWorldText('mix_custom', 'Custom'));
  return picked.join(' · ');
}

function previewCaption(card) {
  const name = previewName(card);
  const state = card.classList.contains('is-updating')
    ? oneWorldText('preview_updating', 'Updating…')
    : card.dataset.note || '';
  card.querySelector('figcaption').textContent = state ? name + ' · ' + state : name;
}

function refreshOptionPreviews() {
  showAutoRealm();
  document.querySelectorAll('.option-preview[data-for]').forEach((card) => {
    const img = card.querySelector('img');
    if (!document.getElementById(card.dataset.for) || !img) return;
    // No shipped picture: keep the last one (at first, the page's own) under
    // the live render.
    const key = blockPictureKey(card);
    card.dataset.exact = key ? '1' : '';
    const src = key ? 'images/previews/' + key : (card.dataset.static || img.getAttribute('src'));
    // A live render stands until its group changes; the group's own refresh
    // puts the shipped picture back when that is exact again.
    if (card.dataset.static !== src) {
      card.dataset.static = src;
      if (!card.dataset.live) card.dataset.shown = src;
    }
    paintPreview(card);
    previewCaption(card);
  });
  refreshLivePreviews();
}

const flagName = (flag) => flag.slice(2).split('=')[0];

// Per group: debounced, one request in flight that counts, and a sequence
// number so an answer to an older combination is dropped.
function refreshLivePreviews() {
  const flags = advancedFeatureArgs().flags;
  const offline = document.getElementById('offline-toggle').checked;
  Object.entries(PREVIEW_GROUPS).forEach(([group, names]) => {
    const mine = flags.filter((f) => names.includes(flagName(f)));
    const state = livePreviews[group] || (livePreviews[group] = { key: null, seq: 0, timer: null });
    const key = mine.join(' ') + (offline ? ' offline' : '');
    if (key === state.key) return;
    state.key = key;
    const seq = ++state.seq;
    clearTimeout(state.timer);
    const cards = Array.from(document.querySelectorAll('.option-preview[data-group="' + group + '"]'));
    const exact = cards.every((c) => c.dataset.exact === '1');
    const settle = (card) => {
      delete card.dataset.live;
      delete card.dataset.note;
      card.classList.remove('is-updating');
      card.dataset.shown = card.dataset.static;
      paintPreview(card);
      previewCaption(card);
    };
    if (mine.length === 0 || exact) {
      cards.forEach(settle);
      return;
    }
    // A frame whose own options match a shipped picture keeps it.
    const live = cards.filter((c) => c.dataset.exact !== '1');
    cards.filter((c) => !live.includes(c)).forEach(settle);
    live.forEach((card) => { card.classList.add('is-updating'); previewCaption(card); });
    state.timer = setTimeout(async () => {
      let src = null;
      let note = '';
      try {
        src = await invoke('gui_render_preview', { group, flags: mine, offline });
      } catch (error) {
        if (String(error).includes('needs-data')) {
          note = oneWorldText('preview_needs_data', 'Preview needs data');
        } else {
          console.warn('Live preview failed:', error);
        }
      }
      if (seq !== state.seq) return;
      live.forEach((card) => {
        card.classList.remove('is-updating');
        // A failed render shows the shipped picture, not an older combination's.
        if (src) card.dataset.live = '1';
        else delete card.dataset.live;
        card.dataset.shown = src || card.dataset.static;
        paintPreview(card);
        if (note) card.dataset.note = note;
        else delete card.dataset.note;
        previewCaption(card);
      });
    }, 600);
  });
}

// The prop families live in one hidden field, the comma list --props takes,
// so the store keeps them as one value; the checkboxes only edit it.
function initPropFamilies() {
  const field = document.getElementById('props-custom-input');
  const boxes = Array.from(document.querySelectorAll('#props-custom-row input[data-prop]'));
  const show = () => {
    const picked = field.value.split(',');
    boxes.forEach((box) => { box.checked = picked.includes(box.dataset.prop); });
  };
  boxes.forEach((box) => box.addEventListener('change', () => {
    field.value = boxes.filter((b) => b.checked).map((b) => b.dataset.prop).join(',');
    fireInputChange(field);
  }));
  field.addEventListener('change', show);
  show();
}

// Tree Pack Folder: what gui_tree_pack_status last found there (the field's
// folder, or the default next to Arnis when it is empty).
let treePack = null;
let treePackSeq = 0;

async function refreshTreePack() {
  const seq = ++treePackSeq;
  const field = document.getElementById('tree-pack-dir-input');
  let status = null;
  try {
    status = await invoke('gui_tree_pack_status', { folder: field.value });
  } catch (error) {
    console.warn('Tree pack status failed:', error);
  }
  if (seq !== treePackSeq) return;
  treePack = status;
  if (status) {
    field.placeholder = status.default_folder;
    setFeatureNotice('tree-pack-status', status.exists
      ? oneWorldText('tree_pack_found', '{n} custom trees found ({m} skipped)', { n: status.found, m: status.skipped })
      : oneWorldText('tree_pack_missing', 'No folder there yet. Create Folder Structure makes it.'));
  }
  refreshLivePreviews();
}

function initTreePack() {
  const field = document.getElementById('tree-pack-dir-input');
  bindBrowse('tree-pack-dir-browse', field, 'gui_pick_save_directory',
    () => field.value.trim() || (treePack && treePack.folder) || '', 'startPath');
  field.addEventListener('change', refreshTreePack);
  [['tree-pack-create-button', false], ['tree-pack-export-button', true]].forEach(([id, exporting]) => {
    const button = document.getElementById(id);
    button.addEventListener('click', async () => {
      button.disabled = true;
      try {
        await invoke('gui_tree_pack_layout', { folder: field.value, export: exporting });
        await refreshTreePack();
      } catch (error) {
        setFeatureNotice('tree-pack-status', String(error), false);
      } finally {
        refreshAdvancedFeatures();
      }
    });
  });
  refreshTreePack();
}

// A notice under a button: what the click did, green when it worked.
function setFeatureNotice(id, text, ok) {
  const el = document.getElementById(id);
  if (!el) return;
  el.style.display = text ? '' : 'none';
  const slot = el.querySelector('span') || el;
  slot.textContent = text || '';
  el.classList.toggle('is-success', ok === true);
  el.classList.toggle('is-error', ok === false);
}

function initExperimentalButtons() {
  const preview = document.getElementById('climate-preview-button');
  const image = document.getElementById('climate-preview-image');
  preview.addEventListener('click', async () => {
    if (!selectedBBox) {
      image.removeAttribute('src');
      setFeatureNotice('climate-preview', oneWorldText('select_location_first', 'Select an area on the map first.'), false);
      return;
    }
    preview.disabled = true;
    try {
      image.src = await invoke('gui_climate_preview', { bboxText: selectedBBox });
      setFeatureNotice('climate-preview', oneWorldText('climate_preview_done', 'Climate zones of the selected area.'), true);
    } catch (error) {
      image.removeAttribute('src');
      setFeatureNotice('climate-preview', String(error), false);
    } finally {
      preview.disabled = false;
    }
  });

  const redraw = document.getElementById('redraw-map-button');
  redraw.addEventListener('click', async () => {
    redraw.disabled = true;
    try {
      const id = await invoke('gui_redraw_one_world_map', { savePath: savePath, worldName: oneWorldFolderName() });
      setFeatureNotice('redraw-map-status', oneWorldText('redraw_map_done', 'Map #{id} now shows every area.', { id }), true);
    } catch (error) {
      setFeatureNotice('redraw-map-status', String(error), false);
    } finally {
      refreshAdvancedFeatures();
    }
  });
}

const formatPercent = (v) => Math.round(v) + '%';
const MELD_SLIDERS = [
  ['snow-percent-slider', formatPercent],
  ['rock-density-slider', (v) => v.toFixed(2)],
  ['bush-density-slider', (v) => v.toFixed(2)],
  ['field-scale-slider', formatPercent],
];

// Meld Generation rows that only depend on the master switch.
const MELD_ALWAYS = [
  'snow-mode-select', 'rocks-toggle', 'bushes-toggle', 'road-detail-select', 'no-buildings-toggle',
  'field-mix-select', 'farm-crops-input', 'tree-realm-select', 'river-bed-select', 'water-detail-select',
  ...TREE_SIZES.map((size) => 'tree-weight-' + size + '-slider'),
  'climate-mode-select', 'climate-preview-button', 'grass-texture-toggle', 'land-texture-toggle',
  'world-seed-input', 'props-select',
  'tree-pack-dir-input', 'tree-pack-mode-select', 'tree-pack-create-button', 'tree-pack-export-button',
];

function formatCpuUsage() {
  const cpu = document.getElementById('cpu-usage-slider');
  const out = document.getElementById('cpu-usage-value');
  if (!cpu || !out) return;
  const pct = parseInt(cpu.value, 10) || 0;
  out.textContent = pct > 0 ? pct + '%' : ((window.localization && window.localization.features_auto) || 'Auto');
}

// One place decides every row, so the master switch, the CPU Usage / Threads
// exclusion and the One World gate never fight over `disabled`.
function refreshAdvancedFeatures() {
  const master = document.getElementById('advanced-features-toggle');
  const groups = document.getElementById('advanced-features-groups');
  if (!master || !groups) return;
  const on = master.checked;
  groups.style.display = on ? '' : 'none';
  document.getElementById('advanced-features-end-toggle').checked = on;
  placeOfflinePanel();
  refreshDataPlan();
  const cpuSet = (parseInt(document.getElementById('cpu-usage-slider').value, 10) || 0) > 0;
  const threadsSet = (parseInt(document.getElementById('threads-input').value, 10) || 0) > 0;
  // Threads wins when both hold a value (only reachable from stored state),
  // so one of the two always stays editable.
  setSettingsRowAvailable('cpu-usage-slider', on && !threadsSet);
  setSettingsRowAvailable('threads-input', on && (threadsSet || !cpuSet));
  setSettingsRowAvailable('ram-budget-input', on);
  setSettingsRowAvailable('max-downloads-input', on);
  // Big Worlds builds in pieces with or without One World (a large selection
  // becomes one), so these follow it alone.
  setSettingsRowAvailable('big-worlds-toggle', on);
  const big = on && document.getElementById('big-worlds-toggle').checked;
  setSettingsRowAvailable('one-world-workers-select', big);
  setSettingsRowAvailable('unit-regions-select', big);
  setSettingsRowAvailable('snap-mode-select', big);
  setSettingsRowAvailable('square-selection-toggle', big);

  MELD_ALWAYS.forEach((id) => setSettingsRowAvailable(id, on));
  const checked = (id) => {
    const el = document.getElementById(id);
    return el.checked && !el.disabled;
  };
  // The snow rows mean nothing in the other modes, so they hide; the share
  // is greyed under One World, which refuses peaks.
  const snow = document.getElementById('snow-mode-select').value;
  const shown = (id, show) => {
    document.getElementById(id).style.display = show ? '' : 'none';
  };
  shown('snow-percent-row', snow === 'peaks');
  shown('snow-y-row', snow === 'manual');
  setSettingsRowAvailable('snow-percent-slider', on && snow === 'peaks' && !isOneWorldEnabled());
  setSettingsRowAvailable('snow-y-input', on && snow === 'manual');
  setSettingsRowAvailable('rock-density-slider', on && checked('rocks-toggle'));
  setSettingsRowAvailable('bush-density-slider', on && checked('bushes-toggle'));
  setSettingsRowAvailable('loot-table-input', on && checked('interior-toggle'));
  // Parcels exist with a layout other than Classic, or with farm crops.
  const parcels = document.getElementById('field-mix-select').value !== 'classic'
    || document.getElementById('farm-crops-input').value.trim() !== '';
  setSettingsRowAvailable('field-scale-slider', on && parcels);
  setSettingsRowAvailable('cave-seed-input', on && checked('caves-toggle'));
  setSettingsRowAvailable('cave-datum-y-input', on && checked('caves-toggle'));
  ['cave-style-select', 'cave-ores-select', 'cave-biomes-input'].forEach((id) => {
    setSettingsRowAvailable(id, on && checked('caves-toggle'));
  });

  // Experimental. A One World merges into Anvil files and fixes its own build
  // height, so the region format and the floor and ceiling are a single
  // Java world's.
  const single = on && selectedWorldFormat === 'java' && !isOneWorldEnabled();
  setSettingsRowAvailable('region-format-select', single);
  const blinear = single && document.getElementById('region-format-select').value === 'blinear';
  shown('blinear-level-row', blinear);
  setSettingsRowAvailable('blinear-level-input', blinear);
  setSettingsRowAvailable('grass-mix-input', on && (checked('grass-texture-toggle') || checked('land-texture-toggle')));
  setSettingsRowAvailable('land-mix-input', on && checked('land-texture-toggle'));
  const tall = single && checked('disable-height-limit-toggle');
  setSettingsRowAvailable('world-floor-input', tall);
  setSettingsRowAvailable('world-ceiling-input', tall);
  const props = document.getElementById('props-select').value;
  shown('props-custom-row', props === 'custom');
  setSettingsRowAvailable('props-custom-input', on && props === 'custom');
  setSettingsRowAvailable('props-min-scale-input', on && props !== 'none');
  setSettingsRowAvailable('redraw-map-button', on && isOneWorldEnabled());
  setSettingsRowAvailable('world-border-toggle', on && selectedWorldFormat === 'java');
  refreshSnapPreview();

  // OSM Data Source, not behind the switch: each source shows its own field.
  const source = document.getElementById('osm-source-select').value;
  [['osm-tiles-url', 'archive'], ['overpass-url', 'overpass'], ['osm-file', 'file'], ['osm-pbf', 'pbf'],
    ['local-archive', 'local'], ['arnis-tiles-path', 'local']].forEach(([id, value]) => {
    shown(id + '-row', source === value);
    setSettingsRowAvailable(id + '-input', source === value);
  });
  shown('osm-pbf-bake-row', source === 'pbf');
  // Pieces exist only with the switch and One World; offline has nothing to warm.
  setSettingsRowAvailable('prewarm-first-toggle',
    on && isOneWorldEnabled() && !checked('offline-toggle'));
  refreshSettingsState();
}

// The Extra Features as CLI flags for gui_start_generation, which parses
// them with the CLI's own parser. A control that is disabled or on its default
// adds none, so the run is the stock one unless a control says otherwise.
function advancedFeatureArgs() {
  const enabled = (id) => {
    const el = document.getElementById(id);
    return el && !el.disabled ? el : null;
  };
  const positive = (id) => {
    const el = enabled(id);
    const n = el ? parseInt(el.value, 10) : NaN;
    return n > 0 ? n : null;
  };
  const changed = (id) => {
    const el = enabled(id);
    if (!el) return null;
    if (el.tagName === 'SELECT') {
      const def = Array.from(el.options).find((o) => o.defaultSelected) || el.options[0];
      return el.value !== def.value ? el.value : null;
    }
    const n = parseFloat(el.value);
    return Number.isFinite(n) && n !== parseFloat(el.defaultValue) ? n : null;
  };
  const on = (id) => (enabled(id) && enabled(id).checked ? true : null);
  const text = (id) => {
    const el = enabled(id);
    return el && el.value.trim() !== '' ? el.value.trim() : null;
  };
  const int = (id) => {
    const t = text(id);
    return t === null ? null : parseInt(t, 10);
  };
  const weights = TREE_SIZES.map((size) => enabled('tree-weight-' + size + '-slider'));
  const weighted = weights.some((el) => el && parseFloat(el.value) !== 100);
  const treePackDir = enabled('tree-pack-dir-input') && treePack && treePack.exists ? treePack.folder : null;
  const workers = enabled('one-world-workers-select');
  const propsSelect = enabled('props-select');
  // Auto leaves the props to the 3D Models switch, as stock.
  const props = propsSelect && propsSelect.value !== 'auto' ? propsSelect.value : null;
  const source = document.getElementById('osm-source-select').value;
  const overpass = (text('overpass-url-input') || '').split(',').map((u) => u.trim()).filter(Boolean).join(',');
  const values = {
    'cpu-target': positive('cpu-usage-slider'),
    'threads': positive('threads-input'),
    'ram-budget-mb': positive('ram-budget-input'),
    'max-downloads': positive('max-downloads-input'),
    // Either one builds a One World area in pieces; a single run ignores both.
    'one-world-workers': workers ? workers.value : null,
    'unit-regions': positive('unit-regions-select'),
    'snow-mode': changed('snow-mode-select'),
    'snow-percent': changed('snow-percent-slider'),
    // Manual needs a line, so it goes even on its default.
    'snow-y': int('snow-y-input'),
    'road-detail': changed('road-detail-select'),
    'rocks': on('rocks-toggle'),
    'rock-density': changed('rock-density-slider'),
    'bushes': on('bushes-toggle'),
    'bush-density': changed('bush-density-slider'),
    'no-buildings': on('no-buildings-toggle'),
    'loot-table': text('loot-table-input'),
    'field-mix': changed('field-mix-select'),
    'farm-crops': text('farm-crops-input'),
    'field-scale': changed('field-scale-slider'),
    'tree-realm': changed('tree-realm-select'),
    'tree-size-weights': weighted
      ? TREE_SIZES.map((size, i) => size + '=' + parseFloat(weights[i].value)).join(',')
      : null,
    // Only a folder that is there; an empty field means the default one.
    'tree-pack-dir': treePackDir,
    'tree-pack-mode': treePackDir ? changed('tree-pack-mode-select') : null,
    // A string, so a seed past 2^53 reaches the parser whole.
    'cave-seed': text('cave-seed-input'),
    'cave-datum-y': int('cave-datum-y-input'),
    'cave-style': changed('cave-style-select'),
    'cave-ores': changed('cave-ores-select'),
    'cave-biomes': text('cave-biomes-input'),
    'river-bed': changed('river-bed-select'),
    'water-detail': changed('water-detail-select'),
    'region-format': changed('region-format-select'),
    'blinear-level': changed('blinear-level-input'),
    'climate-mode': changed('climate-mode-select'),
    'grass-texture': on('grass-texture-toggle'),
    'grass-mix': text('grass-mix-input'),
    'land-texture': on('land-texture-toggle'),
    'land-mix': text('land-mix-input'),
    'min-y': int('world-floor-input'),
    'max-y': int('world-ceiling-input'),
    'seed': text('world-seed-input'),
    // Custom with nothing ticked places none.
    'props': props === 'custom' ? (text('props-custom-input') || 'none') : props,
    'props-min-scale': text('props-min-scale-input'),
    'world-border': on('world-border-toggle'),
    // OSM Data Source: sent whatever the Extra Features switch says.
    'no-tile-archive': source === 'overpass' ? true : null,
    // Local Archive reads the baked folder through the same flag.
    'osm-tiles-url': source === 'local' ? localArchiveFolder() : text('osm-tiles-url-input'),
    'overpass-url': overpass || null,
    'file': text('osm-file-input'),
    // Region Download: an empty file field picks the Geofabrik extract.
    'osm-pbf': source === 'pbf' ? (text('osm-pbf-input') || 'geofabrik') : null,
    'offline': on('offline-toggle'),
    'prewarm-first': on('prewarm-first-toggle'),
  };
  const flags = Object.entries(values)
    .filter(([, v]) => v !== null)
    .map(([name, v]) => (v === true ? '--' + name : '--' + name + '=' + v));
  return { flags };
}

// Caves are carved into the filled ground, so turning them on turns Fill Ground
// on, and turning Fill Ground off takes the caves with it.
function initCavesFillCoupling() {
  const caves = document.getElementById('caves-toggle');
  const fill = document.getElementById('fillground-toggle');
  if (!caves || !fill) return;

  // Dispatch so the settings store persists the knock-on change too.
  const set = (el, value) => {
    if (el.checked === value) return;
    el.checked = value;
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };

  caves.addEventListener('change', () => {
    if (caves.checked) set(fill, true);
  });
  fill.addEventListener('change', () => {
    if (!fill.checked) set(caves, false);
  });
}

function initWorldFormatToggle() {
  initLuantiExperimentalToggle();

  const savedFormat = localStorage.getItem('arnis-world-format');
  if (savedFormat && VALID_FORMATS.includes(savedFormat)) {
    selectedWorldFormat = savedFormat;
  }
  if (selectedWorldFormat === 'luanti' && !isLuantiEnabled()) {
    selectedWorldFormat = 'java';
  }

  updateFormatToggleUI(selectedWorldFormat);
}

function isLuantiEnabled() {
  return localStorage.getItem('arnis-luanti-enabled') === 'true';
}

function initLuantiExperimentalToggle() {
  const toggle = document.getElementById('enable-luanti-toggle');
  const luantiBtn = document.getElementById('format-luanti');
  const bedrockBtn = document.getElementById('format-bedrock');
  if (!toggle || !luantiBtn) return;

  const luantiPathRow = document.getElementById('luanti-save-path-row');

  const applyRightmost = (enabled) => {
    luantiBtn.style.display = enabled ? '' : 'none';
    luantiBtn.classList.toggle('format-toggle-btn--rightmost', enabled);
    if (bedrockBtn) {
      bedrockBtn.classList.toggle('format-toggle-btn--rightmost', !enabled);
    }
    // A save path for a format nobody can pick is only noise.
    if (luantiPathRow) luantiPathRow.style.display = enabled ? '' : 'none';
  };

  const enabled = isLuantiEnabled();
  toggle.checked = enabled;
  applyRightmost(enabled);

  toggle.addEventListener('change', () => {
    const on = toggle.checked;
    localStorage.setItem('arnis-luanti-enabled', on ? 'true' : 'false');
    applyRightmost(on);
    if (!on && selectedWorldFormat === 'luanti') {
      setWorldFormat('java');
    }
  });
}

function setWorldFormat(format) {
  if (!VALID_FORMATS.includes(format)) return;
  if (format === 'luanti' && (!isLuantiEnabled() || isOneWorldToggleOn())) return;

  selectedWorldFormat = format;
  localStorage.setItem('arnis-world-format', format);
  updateFormatToggleUI(format);
}

function getEffectiveWorldFormat() {
  if (selectedWorldFormat === 'luanti') {
    return 'luanti_mineclonia';
  }
  return selectedWorldFormat;
}

// The extended dimension is declared by the Java datapack and the Bedrock
// behavior pack; Luanti ships neither, and off Earth the relief already fits
// vanilla height, so the backend forces the flag off there.
function heightLimitAvailable(format) {
  return selectedCelestialBody === 'earth' && format !== 'luanti';
}

function refreshHeightLimitRow(format) {
  const toggle = document.getElementById('disable-height-limit-toggle');
  if (!toggle) return;

  const available = heightLimitAvailable(format || selectedWorldFormat);
  toggle.disabled = !available;

  const row = toggle.closest('.settings-row');
  if (row) {
    // Cleared, not set to 1: an inline value would beat the class rule.
    row.style.opacity = '';
    row.classList.toggle('settings-row-unavailable', !available);
  }
}

// Bedrock and Luanti keep their own flat generators, so the choice only means
// something for Java. Greyed rather than hidden, like the other format gates.
function refreshWorldTypeRow(format) {
  const group = document.getElementById('world-type-group');
  if (!group) return;
  const java = (format || selectedWorldFormat) === 'java';
  group.classList.toggle('segmented-disabled', !java);
  const row = group.closest('.settings-row');
  if (row) row.classList.toggle('settings-row-unavailable', !java);
}

function updateFormatToggleUI(format) {
  const javaBtn = document.getElementById('format-java');
  const bedrockBtn = document.getElementById('format-bedrock');
  const luantiBtn = document.getElementById('format-luanti');

  refreshHeightLimitRow(format);
  refreshWorldTypeRow(format);

  javaBtn.classList.remove('format-active');
  bedrockBtn.classList.remove('format-active');
  if (luantiBtn) luantiBtn.classList.remove('format-active');

  if (format === 'java') {
    javaBtn.classList.add('format-active');
  } else if (format === 'bedrock') {
    bedrockBtn.classList.add('format-active');
    // Clear world path for bedrock (auto-generated)
    worldPath = "";
  } else if (format === 'luanti') {
    if (luantiBtn) luantiBtn.classList.add('format-active');
    worldPath = "";
  }
  // The label names the last Java world only while that is still the target.
  if (!worldPath) worldLabelName = "";

  // The facade panels are Java entities, so the mode control changes with the
  // format. Called from here so a format picked before the settings modal is
  // ever opened still leaves it consistent.
  refreshFacadeRows();
  // Custom names are Java-only; hide/show the pencil and re-derive the
  // label preview whenever the active format changes.
  refreshWorldNameEditUI();
  // One World is Java-only too: its status and pins follow the format.
  refreshOneWorldState();
}

// Expose to window for onclick handlers
window.setWorldFormat = setWorldFormat;

// The backend only reports crashes and errors while it holds a consent of true,
// so push the stored answer to it at startup and on every change.
function syncTelemetryConsent() {
  const consent = localStorage.getItem('telemetry-consent') === 'true';
  Promise.resolve(invoke('gui_set_telemetry_consent', { consent })).catch(() => {});
}

// Telemetry consent (first run only)
function initTelemetryConsent() {
  const key = 'telemetry-consent'; // values: 'true' | 'false'
  const existing = localStorage.getItem(key);

  const modal = document.getElementById('telemetry-modal');
  if (!modal) return;

  if (existing === null) {
    // First run: ask for consent
    showModal(modal);
  }
  syncTelemetryConsent();

  // Expose handlers
  window.acceptTelemetry = () => {
    localStorage.setItem(key, 'true');
    hideModal(modal);
    syncTelemetryConsent();
    // Update settings toggle to reflect the consent
    const telemetryToggle = document.getElementById('telemetry-toggle');
    if (telemetryToggle) {
      telemetryToggle.checked = true;
    }
    // Set from code, so no change event fired.
    refreshSettingsState();
  };

  window.rejectTelemetry = () => {
    localStorage.setItem(key, 'false');
    hideModal(modal);
    syncTelemetryConsent();
    // Update settings toggle to reflect the consent
    const telemetryToggle = document.getElementById('telemetry-toggle');
    if (telemetryToggle) {
      telemetryToggle.checked = false;
    }
    refreshSettingsState();
  };

  // Utility for other scripts to read consent
  window.getTelemetryConsent = () => {
    const v = localStorage.getItem(key);
    return v === null ? null : v === 'true';
  };
}

// Wires the "Clear Tile Cache" button in the Application settings panel
// to the Rust-side `gui_clear_tile_caches` command. User feedback is a
// brief background flash (green on success, red on partial failure) —
// keeps the row visually consistent with the other checkbox/slider
// rows, no extra status label. The button stays disabled while the
// call is in flight so repeated clicks can't fire multiple concurrent
// wipes (Rust is idempotent, but the UI would look confused).
// How much disk the caches hold, shown next to the Clear button so the user
// can see whether clearing is worth doing. Asked for when the settings panel
// opens and again after anything that changes the caches, never at startup and
// never on a timer: the answer is a walk of every cached file, which is tenths
// of a second once a facade run has filled the tile cache.
async function refreshCacheSize() {
  const label = document.getElementById('cache-size');
  if (!label) {
    return;
  }
  try {
    label.textContent = await invoke('gui_get_cache_size');
  } catch (error) {
    console.warn('Cache size unavailable:', error);
    label.textContent = '';
  }
}
window.refreshCacheSize = refreshCacheSize;

function initClearCacheButton() {
  const button = document.getElementById('clear-cache-button');
  if (!button) {
    return;
  }
  // Deliberately not asking for the size here. This runs on DOMContentLoaded,
  // where the number cannot be seen by anyone: the label lives inside the
  // settings panel, and `openSettings` asks for it there. Reading it at startup
  // only bought a value that was stale by the time the panel opened, and paid
  // for it with a walk of every cached file while the window was going up.

  // How long the success/error flash stays applied before reverting to
  // the default outline. Long enough to register as confirmation, short
  // enough that a user can click again quickly if they want.
  const FLASH_MS = 1500;
  let flashTimer = null;

  const flash = (cls) => {
    button.classList.remove('is-success', 'is-error');
    button.classList.add(cls);
    if (flashTimer) {
      clearTimeout(flashTimer);
    }
    flashTimer = setTimeout(() => {
      button.classList.remove('is-success', 'is-error');
      flashTimer = null;
    }, FLASH_MS);
  };

  button.addEventListener('click', async () => {
    if (button.disabled) {
      return;
    }
    button.disabled = true;
    // Pre-emptively drop any lingering flash class from a previous run
    // so "clearing…" state isn't tinted green/red left over from before.
    button.classList.remove('is-success', 'is-error');
    try {
      await invoke('gui_clear_tile_caches');
      flash('is-success');
      refreshCacheSize();
    } catch (error) {
      // The Rust side returns Err(String) for partial failures (files
      // still locked). The user sees the red flash; the full text goes
      // to the browser console for debugging, not the UI.
      console.warn('Clear tile cache failed:', error);
      flash('is-error');
    } finally {
      button.disabled = false;
    }
  });
}

/* Precompute: fills the Mapillary facade cache for the selected area, so the
   generation that follows does no image work and the 3D preview can show the
   walls. The backend refuses a second precompute and one started beside a
   generation; the button state here is that same rule said early, so pressing
   it is never the way to find out it cannot run. */

let precomputeRunning = false;
let precomputeStartedAt = 0;
let precomputeTicker = null;
// The pipeline reports its stages on the shared progress channel. Nothing else
// is emitting while a precompute holds the process, so those lines are mirrored
// into the settings row instead of being left in a status bar the user is not
// looking at, under a progress bar that is not moving.
let precomputeStage = "";

// What the pipeline prefixes its stage lines with. Kept short because the same
// lines go to the progress bar's status line, which is one line of a 320px
// panel.
const FACADE_STAGE_PREFIX = "Facades:";

function setPrecomputeStatus(text, kind, detail) {
  const el = document.getElementById('facade-precompute-status');
  if (!el) return;
  el.textContent = text || "";
  el.style.display = text ? "" : "none";
  el.classList.toggle('is-success', kind === 'success');
  el.classList.toggle('is-error', kind === 'error');
  if (detail) {
    el.title = detail;
  } else {
    el.removeAttribute('title');
  }
}

// Why the button cannot be pressed, or "" when it can. The wording is what the
// button's tooltip says, so a disabled button always explains itself.
function precomputeBlockedReason() {
  if (selectedCelestialBody !== 'earth') {
    return "Facades are Earth only. Switch the world back in the map toolbar.";
  }
  // The source control decides this, and it is the reason the row above greys
  // the button out; without it here the next refresh switches it back on.
  if (getEffectiveFacadeSource() !== 'mapillary') {
    return "Set Facade Source to Mapillary first.";
  }
  if (!getMapillaryToken()) return "Add a Mapillary token above first.";
  if (!selectedBBox || selectedBBox === "0.000000 0.000000 0.000000 0.000000") {
    return "Select an area on the map first.";
  }
  if (!generationButtonEnabled) return "A generation is running.";
  return "";
}

function refreshPrecomputeButton() {
  const button = document.getElementById('precompute-facades-button');
  if (!button) return;

  if (precomputeRunning) {
    // Never disabled while running: this is the only way to stop it.
    button.disabled = false;
    button.textContent = "Cancel";
    button.title = "Stops at the end of the stage it is in. Walls already built stay cached.";
    return;
  }

  const localized = window.localization || {};
  button.textContent = localized['facade_precompute_button'] || "Precompute";
  const blocked = precomputeBlockedReason();
  button.disabled = !!blocked;
  button.title = blocked;
}

// mm:ss since the run started, for a job whose stages are minutes long.
function precomputeElapsed() {
  const seconds = Math.max(0, Math.round((Date.now() - precomputeStartedAt) / 1000));
  return Math.floor(seconds / 60) + ":" + String(seconds % 60).padStart(2, '0');
}

function showPrecomputeProgress() {
  if (!precomputeRunning) return;
  setPrecomputeStatus((precomputeStage || "Working...") + " (" + precomputeElapsed() + ")");
}

// Called by the progress listener for every line the facade pipeline emits, so
// the row says which stage is running rather than only that something is.
function notePrecomputeStage(message) {
  if (!precomputeRunning || !message.startsWith(FACADE_STAGE_PREFIX)) return;
  precomputeStage = message.slice(FACADE_STAGE_PREFIX.length).trim();
  showPrecomputeProgress();
}

function initPrecomputeFacadesButton() {
  const button = document.getElementById('precompute-facades-button');
  if (!button) return;
  refreshPrecomputeButton();

  button.addEventListener('click', async () => {
    if (precomputeRunning) {
      // The pipeline checks between stages, so this is a request, not a stop.
      precomputeStage = "Cancelling after this stage";
      showPrecomputeProgress();
      try {
        await invoke('gui_cancel_precompute');
      } catch (error) {
        console.warn('Cancel precompute failed:', error);
      }
      return;
    }

    const blocked = precomputeBlockedReason();
    if (blocked) {
      setPrecomputeStatus(blocked, 'error');
      return;
    }

    precomputeRunning = true;
    precomputeStartedAt = Date.now();
    precomputeStage = "Starting";
    refreshPrecomputeButton();
    showPrecomputeProgress();
    // One second, so a run that spends twenty minutes in one stage still shows
    // something moving and cannot be mistaken for a hang.
    precomputeTicker = setInterval(showPrecomputeProgress, 1000);

    try {
      const outcome = await invoke('gui_precompute_facades', {
        bboxText: selectedBBox,
        mapillaryToken: getMapillaryToken(),
      });
      // Green only when there are facades here now. A cancelled run and an
      // area with nothing to find both come back plain: neither is a failure
      // and neither left a wall behind.
      setPrecomputeStatus(outcome.summary, outcome.built ? 'success' : '', outcome.detail);
      if (outcome.built) window.arnisPreview3D?.refreshFacades();
      refreshCacheSize();
    } catch (error) {
      // Every refusal from the backend is a sentence meant to be read. The
      // longer ones are written as that sentence, a blank line, and the reason
      // behind it: the row holds one line, so the reason becomes its tooltip.
      const text = String(error);
      const split = text.indexOf("\n\n");
      const head = split < 0 ? text : text.slice(0, split).trim();
      const rest = split < 0 ? "" : text.slice(split + 2).trim();
      setPrecomputeStatus(head, 'error', rest || undefined);
      refreshCacheSize();
    } finally {
      clearInterval(precomputeTicker);
      precomputeTicker = null;
      precomputeRunning = false;
      refreshPrecomputeButton();
    }
  });
}

// Single shared tooltip element appended to <body>, so it escapes the
// `.settings-scrollable` container's `overflow: hidden` clip and can
// extend past the top / sides of the panel. Previously the tooltip
// lived as a `::after` pseudo-element on each `.tooltip-icon`, which
// meant long text or icons near an edge got cut off by the scroll
// container. This global element is positioned via
// `getBoundingClientRect` on hover and auto-flips above ↔ below when
// close to the viewport edge.
function initTooltips() {
  const tooltip = document.createElement('div');
  tooltip.className = 'global-tooltip';
  tooltip.setAttribute('role', 'tooltip');
  tooltip.setAttribute('aria-hidden', 'true');
  const arrow = document.createElement('div');
  arrow.className = 'global-tooltip-arrow';
  tooltip.appendChild(arrow);
  const body = document.createElement('div');
  body.className = 'global-tooltip-body';
  tooltip.appendChild(body);
  document.body.appendChild(tooltip);

  const VIEWPORT_MARGIN = 8; // px gap between tooltip and viewport edge
  const ICON_GAP = 8; // px gap between tooltip and icon

  let currentIcon = null;

  const position = () => {
    if (!currentIcon) return;
    const iconRect = currentIcon.getBoundingClientRect();
    // Measure after text is set; reset to allow natural width.
    const ttRect = tooltip.getBoundingClientRect();

    // Default: centered above the icon. Flip below when there isn't
    // enough room above (e.g. icon near the top of the settings panel,
    // which is the "cut off at the top" case the user reported).
    const spaceAbove = iconRect.top;
    const flipBelow = spaceAbove < ttRect.height + ICON_GAP + VIEWPORT_MARGIN;
    const top = flipBelow
      ? iconRect.bottom + ICON_GAP
      : iconRect.top - ttRect.height - ICON_GAP;

    // Horizontal: center on the icon, then clamp into the viewport so
    // tooltips near the right edge don't overflow into hidden space.
    const desiredLeft = iconRect.left + iconRect.width / 2 - ttRect.width / 2;
    const maxLeft = window.innerWidth - ttRect.width - VIEWPORT_MARGIN;
    const left = Math.max(VIEWPORT_MARGIN, Math.min(desiredLeft, maxLeft));

    tooltip.style.top = top + 'px';
    tooltip.style.left = left + 'px';

    // Point the arrow back at the icon's center, regardless of the
    // horizontal clamp above, and flip it to the opposite edge when the
    // tooltip opens below the icon.
    const iconCenter = iconRect.left + iconRect.width / 2;
    const arrowLeft = Math.max(8, Math.min(ttRect.width - 8, iconCenter - left));
    arrow.style.left = arrowLeft + 'px';
    tooltip.classList.toggle('flipped', flipBelow);
  };

  const show = (icon) => {
    const text = icon.getAttribute('data-tooltip');
    if (!text) return;
    currentIcon = icon;
    body.textContent = text;
    // Position BEFORE making visible. The tooltip stays `visibility:
    // hidden` (layout-active, paint-inactive) so `getBoundingClientRect`
    // returns the real dimensions, but the user never sees a 0,0 flash
    // between insertion and the first position-frame.
    position();
    tooltip.classList.add('is-visible');
    tooltip.setAttribute('aria-hidden', 'false');
  };

  const hide = () => {
    currentIcon = null;
    tooltip.classList.remove('is-visible', 'flipped');
    tooltip.setAttribute('aria-hidden', 'true');
  };

  const bind = (icon) => {
    // Make `<span class="tooltip-icon">` focusable via Tab so keyboard
    // users can reveal the tooltip. Spans are not focusable by default,
    // so the focus/blur listeners below are dead without this. Done in
    // JS rather than HTML so every icon picks it up automatically and
    // we don't have to keep the 14 call sites in sync. `role="button"`
    // is a reasonable hint for screen readers that this thing is
    // interactive even though it doesn't do anything on click.
    if (icon.tabIndex < 0) {
      icon.tabIndex = 0;
    }
    if (!icon.hasAttribute('role')) {
      icon.setAttribute('role', 'button');
    }
    if (!icon.hasAttribute('aria-label')) {
      const text = icon.getAttribute('data-tooltip');
      if (text) {
        icon.setAttribute('aria-label', text);
      }
    }
    icon.addEventListener('mouseenter', () => show(icon));
    icon.addEventListener('mouseleave', hide);
    // The icon sits inside the setting's <label>, whose click would otherwise
    // flip the switch it belongs to.
    icon.addEventListener('click', (e) => e.preventDefault());
    icon.addEventListener('focus', () => show(icon));
    icon.addEventListener('blur', hide);
    // Escape closes the tooltip while it's focused.
    icon.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        hide();
      }
    });
  };

  document.querySelectorAll('.tooltip-icon').forEach(bind);

  // Reposition on viewport resize / scroll (including inside the
  // settings-scrollable container). Also hide on scroll inside the
  // settings panel, because the icon may have scrolled off-screen
  // and a stale tooltip hovering over the wrong row is worse than
  // hiding eagerly.
  window.addEventListener('resize', () => {
    if (currentIcon) position();
  });
  const scrollable = document.querySelector('.settings-scrollable');
  if (scrollable) {
    scrollable.addEventListener('scroll', hide, { passive: true });
  }
}

/// Save path management, one path per world format
let savePath = "";
let bedrockSavePath = "";
let luantiSavePath = "";

const SAVE_PATHS = {
  java: {
    storageKey: 'arnis-save-path',
    defaultCommand: 'gui_get_default_save_path',
    inputId: 'save-path-input',
    browseId: 'save-path-browse',
    get: () => savePath,
    set: (value) => {
      const changed = savePath !== value;
      savePath = value;
      // The One World lives under the saves path, so a new path is a new world.
      if (changed && typeof refreshOneWorldState === 'function') refreshOneWorldState();
    },
  },
  bedrock: {
    storageKey: 'arnis-bedrock-save-path',
    defaultCommand: 'gui_get_default_bedrock_save_path',
    inputId: 'bedrock-save-path-input',
    browseId: 'bedrock-save-path-browse',
    get: () => bedrockSavePath,
    set: (value) => { bedrockSavePath = value; },
  },
  luanti: {
    storageKey: 'arnis-luanti-save-path',
    defaultCommand: 'gui_get_default_luanti_save_path',
    inputId: 'luanti-save-path-input',
    browseId: 'luanti-save-path-browse',
    get: () => luantiSavePath,
    set: (value) => { luantiSavePath = value; },
  },
};

async function initSavePath() {
  for (const config of Object.values(SAVE_PATHS)) {
    config.set(await resolveStoredSavePath(config));
    const input = document.getElementById(config.inputId);
    if (input) {
      input.value = config.get();
    }
  }
}

async function resolveStoredSavePath({ storageKey, defaultCommand }) {
  const saved = localStorage.getItem(storageKey);
  if (saved) {
    // Validate the saved path still exists (handles upgrades / moved directories)
    try {
      const normalized = await invoke('gui_set_save_path', { path: saved });
      localStorage.setItem(storageKey, normalized);
      return normalized;
    } catch (_) {
      console.warn(`Stored path ${storageKey} no longer valid, re-detecting...`);
      localStorage.removeItem(storageKey);
    }
  }

  try {
    const detected = await invoke(defaultCommand);
    localStorage.setItem(storageKey, detected);
    return detected;
  } catch (error) {
    console.error(`Failed to detect path for ${storageKey}:`, error);
    return "";
  }
}

function initSavePathSetting() {
  for (const config of Object.values(SAVE_PATHS)) {
    initSavePathRow(config);
  }
}

function initSavePathRow({ storageKey, inputId, browseId, get, set }) {
  const input = document.getElementById(inputId);
  if (!input) return;

  input.value = get();

  // Manual text input – validate on change, revert if invalid
  input.addEventListener('change', async () => {
    const newPath = input.value.trim();
    if (!newPath) {
      input.value = get();
      return;
    }

    try {
      const validated = await invoke('gui_set_save_path', { path: newPath });
      set(validated);
      input.value = validated;
      localStorage.setItem(storageKey, validated);
    } catch (_) {
      // Invalid path – silently revert to previous value
      input.value = get();
    }
  });

  // Folder picker button
  const browseBtn = document.getElementById(browseId);
  if (browseBtn) {
    browseBtn.addEventListener('click', async () => {
      try {
        const picked = await invoke('gui_pick_save_directory', { startPath: get() });
        if (picked) {
          set(picked);
          input.value = picked;
          localStorage.setItem(storageKey, picked);
        }
      } catch (error) {
        console.error("Folder picker failed:", error);
      }
    });
  }
}

/**
 * Validates and processes bounding box coordinates input
 * Supports both comma and space-separated formats
 * Updates the map display when valid coordinates are entered
 */
function handleBboxInput() {
  const inputBox = document.getElementById("bbox-coords");
  const bboxSelectionInfo = document.getElementById("bbox-selection-info");

  inputBox.addEventListener("input", function () {
    const input = inputBox.value.trim();

    if (input === "") {
      // Empty input - revert to map selection if available
      customBBoxValid = false;
      bboxInputError = false;
      selectedBBox = mapSelectedBBox;
      
      // Clear the info text only if no map selection exists
      if (!mapSelectedBBox) {
        setBboxSelectionInfo(bboxSelectionInfo, "select_area_prompt", "#ffffff");
      } else {
        // Restore map selection info display but don't update input field
        const [lat1, lng1, lat2, lng2] = mapSelectedBBox.split(" ").map(Number);
        const selectedSize = calculateBBoxSize(lat1, lng1, lat2, lng2);
        displayBboxSizeStatus(bboxSelectionInfo, selectedSize);
      }
      return;
    }

    // Regular expression to validate bbox input (supports both comma and space-separated formats)
    const bboxPattern = /^(-?\d+(\.\d+)?)[,\s](-?\d+(\.\d+)?)[,\s](-?\d+(\.\d+)?)[,\s](-?\d+(\.\d+)?)$/;

    if (bboxPattern.test(input)) {
      const matches = input.match(bboxPattern);

      // Extract coordinates (Lat / Lng order expected)
      const lat1 = parseFloat(matches[1]);
      const lng1 = parseFloat(matches[3]);
      const lat2 = parseFloat(matches[5]);
      const lng2 = parseFloat(matches[7]);

      // Validate latitude and longitude ranges in the expected Lat / Lng order
      if (
        lat1 >= -90 && lat1 <= 90 &&
        lng1 >= -180 && lng1 <= 180 &&
        lat2 >= -90 && lat2 <= 90 &&
        lng2 >= -180 && lng2 <= 180
      ) {
        // Input is valid; trigger the event with consistent comma-separated format
        const bboxText = `${lat1},${lng1},${lat2},${lng2}`;
        window.dispatchEvent(new MessageEvent('message', { data: { bboxText } }));

        // Show the typed bbox on the map. Handed over by message rather than
        // by reloading the frame: setting src and then calling reload() on the
        // same frame raced - reload() runs against the pre-hash URL, so the
        // selection could be dropped and the map came back empty. A reload also
        // throws away every tile already fetched, which is the last thing a slow
        // or filtered connection can afford.
        const mapFrame = getMapFrame();
        if (mapFrame && mapFrame.contentWindow) {
          mapFrame.contentWindow.postMessage({
            type: 'setBbox',
            bounds: [lat1, lng1, lat2, lng2]
          }, '*');
        }

        // Update the info text and mark custom input as valid
        customBBoxValid = true;
        bboxInputError = false;
        selectedBBox = bboxText.replace(/,/g, ' '); // Convert to space format for consistency
        setBboxSelectionInfo(bboxSelectionInfo, "custom_selection_confirmed", "#7bd864");

        // Reset rotation when bbox changes via manual input
        if (typeof window.updateRotation === 'function') {
          window.updateRotation(0);
        }
      } else {
        // Valid numbers but invalid order or range
        customBBoxValid = false;
        // Don't clear selectedBBox - keep map selection if available
        if (!mapSelectedBBox) {
          selectedBBox = "";
        } else {
          selectedBBox = mapSelectedBBox;
        }
        bboxInputError = true;
        setBboxSelectionInfo(bboxSelectionInfo, "error_coordinates_out_of_range", "#fecc44");
      }
    } else {
      // Input doesn't match the required format
      customBBoxValid = false;
      // Don't clear selectedBBox - keep map selection if available
      if (!mapSelectedBBox) {
        selectedBBox = "";
      } else {
        selectedBBox = mapSelectedBBox;
      }
      bboxInputError = true;
      setBboxSelectionInfo(bboxSelectionInfo, "invalid_format", "#fecc44");
    }
    // The Precompute button next to this field turns on the selection, and the
    // field is inside the same panel, so it has to follow every keystroke.
    refreshPrecomputeButton();
    refreshSnapPreview();
    refreshDataPlan();
  });
}

/**
 * Calculates the approximate area of a bounding box in square meters
 * Uses the Haversine formula for geodesic calculations
 * @param {number} lat1 - South latitude
 * @param {number} lng1 - West longitude
 * @param {number} lat2 - North latitude
 * @param {number} lng2 - East longitude
 * @returns {number} Area in square meters
 */
// Radii used to turn a bbox into true ground area for the selected body.
const BODY_RADIUS_M = { earth: 6371000, moon: 1737400, mars: 3396000 };

function calculateBBoxSize(lat1, lng1, lat2, lng2) {
  const toRad = (angle) => (angle * Math.PI) / 180;
  // Real ground, not an Earth-sized overestimate: a lunar box reads 13x too large.
  const R = BODY_RADIUS_M[selectedCelestialBody] || BODY_RADIUS_M.earth;
  // Width at the middle latitude, as the world is built and as the CLI's area_km2 measures.
  const height = R * toRad(lat2 - lat1);
  const width = R * toRad(lng2 - lng1) * Math.cos(toRad((lat1 + lat2) / 2));
  return Math.abs(width * height);
}

/**
 * Normalizes a longitude value to the range [-180, 180]
 * @param {number} lon - Longitude value to normalize
 * @returns {number} Normalized longitude value
 */
function normalizeLongitude(lon) {
  return ((lon + 180) % 360 + 360) % 360 - 180;
}

// Selection-size warnings, in square metres of ground at world scale 1. Measured
// timings, peak memory and world sizes, square selections over central Munich:
//   Earth 9km2 8s/1.1GB/146MB | 25km2 20s/1.7GB/399MB | 85km2 66s/3.6GB/1.3GB
//         150km2 111s/5.1GB/2.3GB, so about 1.6GB plus 0.023GB per km2
//   Moon  2deg 5s/4MB | 5deg 9s/16MB | 10deg 21s/36MB | 20deg 68s/144MB
//   Mars  2deg 5s/4MB | 5deg 9s/16MB | 10deg 20s/36MB | 20deg 51s/100MB
// The Earth tiers guard memory more than the clock: about 4GB, 6GB (fine on an
// 8GB machine) and 13GB. The Moon and Mars tiers land near one, three and nine
// minutes.
const AREA_THRESHOLDS = {
  earth: { extensive: 100e6, large: 200e6, extreme: 500e6 },
  moon: { extensive: 3e11, large: 1e12, extreme: 3e12 },
  mars: { extensive: 1.5e12, large: 5e12, extreme: 1.5e13 }
};

// The run estimate under the progress bar extends the Earth numbers above with
// Phase 6 runs on a 24-thread machine at scale 1:
//   size  8x8 km, 256 regions (251k chunks): 984 MB of regions, so 15.4 MB per
//         km2 of world (Munich above: 15.3-16.2) or 3.84 MB per whole region.
//   time  cold single runs 8 km 34 s, 16 km 114 s, 80 km at scale 0.05 (16 km2
//         of world on 6400 km2 of ground) 77 s: about 7 s, plus 0.42 s per km2
//         of world, plus 0.01 s per km2 of ground for the data. Warm runs were
//         ~15% faster (8 km 28.9 s, 16 km 105.8 s), Munich's dense centre ~1.6x
//         slower, and pieces side by side 1.2-1.5x faster than one run.
// ponytail: one machine's clock; a slower CPU lands past the top of the range.
const EST_MB_PER_WORLD_KM2 = 984 / 64;
const EST_MB_PER_REGION = 984 / 256;
const EST_REGION_KM2 = 0.512 * 0.512;
const EST_FIXED_S = 7;
const EST_S_PER_WORLD_KM2 = 0.42;
const EST_S_PER_GROUND_KM2 = 0.01;

// { mb, lo, hi } for the selection and settings: megabytes of Java regions
// (null for the other formats, which were not measured) and a range of
// seconds. Null off Earth or with nothing selected.
function runEstimate() {
  if (!selectedBBox || selectedCelestialBody !== 'earth') return null;
  const [lat1, lng1, lat2, lng2] = selectedBBox.trim().split(/[,\s]+/).map(Number);
  let groundKm2 = calculateBBoxSize(lat1, lng1, lat2, lng2) / 1e6;
  let worldKm2 = groundKm2 * earthScaleFactor();
  let mb = worldKm2 * EST_MB_PER_WORLD_KM2;
  // In pieces the snap knows the whole regions it builds and the ground they cover.
  const pieced = !!selectionPieces();
  if (pieced) {
    const snap = lastSnap.snap;
    const regions = snap.regions[0] * snap.regions[1];
    mb = regions * EST_MB_PER_REGION;
    worldKm2 = regions * EST_REGION_KM2;
    groundKm2 = snap.size_km[0] * snap.size_km[1];
  }
  if (!(worldKm2 > 0)) return null;
  const t = EST_FIXED_S + EST_S_PER_WORLD_KM2 * worldKm2 + EST_S_PER_GROUND_KM2 * groundKm2;
  return {
    mb: selectedWorldFormat === 'java' ? mb : null,
    lo: (t * 0.85) / (pieced ? 1.5 : 1),
    hi: (t * 1.6) / (pieced ? 1.2 : 1),
  };
}

function formatEstimateSize(mb) {
  if (mb >= 1000) return (mb / 1000).toFixed(mb >= 10000 ? 0 : 1) + ' GB';
  return (mb >= 100 ? Math.round(mb / 10) * 10 : Math.max(1, Math.round(mb))) + ' MB';
}

// Rounded so a range never claims more than it knows: 5 s steps, then minutes,
// then 10 minutes.
function formatEstimateTime(lo, hi) {
  const round = (s) => formatEtaDuration(
    s < 90 ? Math.max(5, Math.round(s / 5) * 5) : s < 3600 ? Math.round(s / 60) * 60 : Math.round(s / 600) * 600);
  const a = round(lo), b = round(hi);
  return a === b ? '~' + a : a + '–' + b;
}

let selectedBBox = "";
let mapSelectedBBox = "";  // Tracks bbox from map selection
let customBBoxValid = false;  // Tracks if custom input is valid
let bboxInputError = false;  // The coordinate field holds input that did not validate

/**
 * Displays the appropriate bbox size status message based on area thresholds
 * @param {HTMLElement} bboxSelectionElement - The element to display the message in
 * @param {number} selectedSize - The calculated bbox area in square meters
 */
function displayBboxSizeStatus(bboxSelectionElement, selectedSize) {
  const t = AREA_THRESHOLDS[selectedCelestialBody] || AREA_THRESHOLDS.earth;
  selectedSize *= earthScaleFactor();
  const pieces = selectionPieces();
  let warned = false;
  // Built in pieces, the size is no longer a worry: say how instead.
  if (pieces) {
    setBboxSelectionInfo(bboxSelectionElement, "area_pieces_info", "#ececec", pieces);
  } else if (selectedSize > t.extreme) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extreme", "#ff4444");
    warned = true;
  } else if (selectedSize > t.large) {
    setBboxSelectionInfo(bboxSelectionElement, "area_too_large", "#fa7878");
    warned = true;
  } else if (selectedSize > t.extensive) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extensive", "#fecc44");
    warned = true;
  } else {
    setBboxSelectionInfo(bboxSelectionElement, "selection_confirmed", "#7bd864");
  }
  renderRunStats();
  const cta = document.getElementById("bbox-features-cta");
  if (cta) cta.style.display = warned && !snapActive() && selectedCelestialBody === "earth" ? "" : "none";
}

// Keys the size status owns, so a late snap may replace them.
const BBOX_SIZE_KEYS = ["area_pieces_info", "area_extreme", "area_too_large", "area_extensive", "selection_confirmed"];

// The warning's way out: open Extra Features with the switch and Big Worlds on.
function useExtraFeatures() {
  window.openSettings();
  ["advanced-features-toggle", "big-worlds-toggle"].forEach((id) => {
    const el = document.getElementById(id);
    if (el && !el.checked) {
      el.checked = true;
      el.dispatchEvent(new Event("change", { bubbles: true }));
    }
  });
  const nav = document.querySelector('.settings-nav-item[data-target="settings-section-features"]');
  if (nav) nav.click();
}
window.useExtraFeatures = useExtraFeatures;

// Blocks, and so memory, grow with the square of the scale; Moon and Mars use a fixed one.
function earthScaleFactor() {
  if (selectedCelestialBody !== 'earth') return 1;
  const s = parseFloat(document.getElementById("scale-value-slider")?.value);
  return isFinite(s) && s > 0 ? s * s : 1;
}

// Re-runs the size status, e.g. after a body switch or scale change moves the tier.
function refreshBboxSelectionInfo() {
  // An error about the typed coordinates stays until the field is fixed.
  if (!mapSelectedBBox || bboxInputError || !BBOX_SIZE_KEYS.includes(currentBboxSelectionKey)) return;
  const [lat1, lng1, lat2, lng2] = mapSelectedBBox.split(" ").map(Number);
  displayBboxSizeStatus(
    document.getElementById("bbox-selection-info"),
    calculateBBoxSize(lat1, lng1, lat2, lng2)
  );
}

// Function to handle incoming bbox data
function displayBboxInfoText(bboxText) {
  // Two producers, two separators: the map posts formatBounds output, which is
  // space separated, while manual coordinate entry synthesizes a comma
  // separated string. Splitting on " " alone turned the manual one into a
  // single NaN, which then got written back over the user's half-typed
  // coordinates as "NaN,NaN,undefined,NaN" - the next keystroke failed the
  // format check and the selection was gone.
  // lat,lng,lat,lng throughout - what formatBounds emits, what the manual
  // input accepts, and what LLBBox::from_str parses on the Rust side. Do not
  // "fix" this to lng-first; the backend has a comment saying the same.
  let [lat1, lng1, lat2, lng2] = bboxText.trim().split(/[,\s]+/).map(Number);

  // Normalize longitudes
  lng1 = parseFloat(normalizeLongitude(lng1).toFixed(6));
  lng2 = parseFloat(normalizeLongitude(lng2).toFixed(6));
  mapSelectedBBox = `${lat1} ${lng1} ${lat2} ${lng2}`;

  // Map selection always takes priority - clear custom input and update selectedBBox
  selectedBBox = mapSelectedBBox;
  customBBoxValid = false;
  bboxInputError = false;

  // Reset rotation when bbox changes
  if (typeof window.updateRotation === 'function') {
    window.updateRotation(0);
  }

  const bboxSelectionInfo = document.getElementById("bbox-selection-info");
  const bboxCoordsInput = document.getElementById("bbox-coords");

  // Reset the info text if the bbox is 0,0,0,0
  if (lat1 === 0 && lng1 === 0 && lat2 === 0 && lng2 === 0) {
    setBboxSelectionInfo(bboxSelectionInfo, "select_area_prompt", "#ffffff");
    bboxCoordsInput.value = "";
    mapSelectedBBox = "";
    if (!customBBoxValid) {
      selectedBBox = "";
    }
    window.arnisPreview3D?.onBboxCleared();
    refreshPrecomputeButton();
    refreshDataPlan();
    return;
  }

  // Update the custom bbox input with the map selection (comma-separated
  // format) - but never type over the user. This also runs as the echo of a
  // bbox they are entering into this very field, and rewriting it mid-edit
  // moves the caret so the next keystroke lands in the wrong place.
  //
  // The echo is caught by value, not by focus: focus can legitimately be
  // elsewhere (the map takes it on interaction, and activeElement is
  // unreliable while the window itself is unfocused) even though the field
  // still holds what the user typed. The focus check then covers the other
  // direction - a genuinely different, map-driven selection arriving while
  // the caret is in the field.
  const current = bboxCoordsInput.value.trim().split(/[,\s]+/).map(Number);
  const echoesField = current.length === 4 && current[0] === lat1 &&
    current[1] === lng1 && current[2] === lat2 && current[3] === lng2;
  if (!echoesField && document.activeElement !== bboxCoordsInput) {
    bboxCoordsInput.value = `${lat1},${lng1},${lat2},${lng2}`;
  }

  // Calculate the size of the selected bbox
  const selectedSize = calculateBBoxSize(lat1, lng1, lat2, lng2);

  displayBboxSizeStatus(bboxSelectionInfo, selectedSize);

  // Hide any rendered mini 3D preview if the selection actually changed
  window.arnisPreview3D?.onBboxChanged(selectedBBox);
  refreshPrecomputeButton();
  refreshSnapPreview();
  refreshDataPlan();
}

/* Large worlds: with pieces in use the selection grows to whole regions of the
   One World's grid, so every piece is whole regions. The Rust side does the
   frame maths, the same as the run's; the map draws the result. */

function extraFeaturesOn() {
  const master = document.getElementById('advanced-features-toggle');
  return !!(master && master.checked);
}

// With Big Worlds on, every selection gets its cells on the map. Off Earth the
// scale slider is not the world's, so there is no grid to place.
function snapActive() {
  const big = document.getElementById('big-worlds-toggle');
  return extraFeaturesOn() && !!(big && big.checked && !big.disabled) && selectedCelestialBody === 'earth';
}

function snapSelection(bbox) {
  return invoke('gui_snap_selection', {
    bboxText: bbox,
    savePath: savePath,
    // null: the new One World a large selection becomes, unless a stopped
    // job of this selection made one: that one, so starting again resumes it.
    worldName: isOneWorldEnabled() ? oneWorldFolderName()
      : resumeWorld && resumeWorld.bbox === bbox ? resumeWorld.name : null,
    scale: parseFloat(document.getElementById('scale-value-slider').value) || 1,
    unitRegions: parseInt(document.getElementById('unit-regions-select').value, 10) || 4,
    snapMode: document.getElementById('snap-mode-select').value,
    square: document.getElementById('square-selection-toggle').checked,
    // Sizes Parallel Workers as the run will.
    flags: advancedFeatureArgs().flags,
  });
}

// Pieces need a One World, so a Java world on Earth. With One World on they
// are always used; without it a selection of more than one cell runs as a new
// One World, and one cell is the usual single run.
function snapRunsAsOneWorld(snap) {
  return isOneWorldEnabled() || (isOneWorldAvailable() && snap.cells[0] * snap.cells[1] > 1);
}

// The bbox a run is given and the world it goes to: the snapped bbox when it
// builds in pieces. A new world is also pinned to the snap's centre, so the
// run's frame is the one the cell lines were placed in.
async function runSelectionFor(bbox) {
  const plain = { bbox, flags: [], oneWorld: isOneWorldEnabled(), worldName: oneWorldFolderName() };
  if (!snapActive()) return plain;
  const snap = await snapSelection(bbox);
  if (!snapRunsAsOneWorld(snap)) return plain;
  return {
    bbox: snap.bbox,
    flags: snap.new_world ? ['--origin=' + snap.origin.join(',')] : [],
    oneWorld: true,
    worldName: isOneWorldEnabled() ? oneWorldFolderName() : snap.world_name,
  };
}

let snapPreviewKey = null;
let snapPreviewTimer = null;
// The last drawn snap and the selection it is for.
let lastSnap = null;
// Debounced: a dragged selection or a slider fires many events, and only the
// last one needs a grid.
function refreshSnapPreview() {
  clearTimeout(snapPreviewTimer);
  snapPreviewTimer = setTimeout(drawSnapPreview, 100);
}

// { n, w } while the current selection is built in more than one piece, so
// the size line can say "Builds in N pieces · W workers"; else null.
function selectionPieces() {
  if (!snapActive() || !lastSnap || lastSnap.bbox !== selectedBBox) return null;
  const snap = lastSnap.snap;
  const n = snap.cells[0] * snap.cells[1];
  return n > 1 && snapRunsAsOneWorld(snap) ? { n, w: snap.workers } : null;
}

async function drawSnapPreview() {
  const on = snapActive() && !!selectedBBox;
  // The map shows its Show grid button while a grid can show.
  postToMap({ type: 'snapGridControl', visible: snapActive() });
  const key = on ? JSON.stringify([selectedBBox, savePath, isOneWorldEnabled(), isOneWorldAvailable(),
    oneWorldFolderName(),
    document.getElementById('scale-value-slider').value,
    document.getElementById('unit-regions-select').value,
    document.getElementById('snap-mode-select').value,
    document.getElementById('square-selection-toggle').checked,
    advancedFeatureArgs().flags]) : 'off';
  if (key === snapPreviewKey) return;
  snapPreviewKey = key;
  let snap = null;
  if (on) {
    try {
      snap = await snapSelection(selectedBBox);
    } catch (error) {
      console.warn('Cell snap failed:', error);
    }
    if (key !== snapPreviewKey) return;
    // A failed snap is asked again on the next change, not never.
    if (!snap) snapPreviewKey = null;
  }
  lastSnap = snap ? { bbox: selectedBBox, snap } : null;
  renderRunStats();
  const how = !snap ? ''
    : !snapRunsAsOneWorld(snap) ? oneWorldText('snap_single_run', 'Builds in one run')
      : isOneWorldEnabled() ? '' : oneWorldText('snap_as_one_world', 'Builds as a One World');
  // Width (east-west) first, then height (north-south).
  const text = snap
    ? oneWorldText('snap_regions_info', '{x} × {z} regions · {cx} × {cz} cells · {pieces} pieces · {w} × {h} km', {
      x: snap.regions[0], z: snap.regions[1], cx: snap.cells[0], cz: snap.cells[1],
      pieces: snap.cells[0] * snap.cells[1], w: snap.size_km[0].toFixed(1), h: snap.size_km[1].toFixed(1),
    }) + (snap.fallback
      ? ' · ' + oneWorldText('snap_fallback', 'No whole cell fits inside, so one is used.')
      : '') + (how ? ' · ' + how : '')
    : '';
  // The map marks the outline's width (W) and height (H) like a drawing.
  const dims = snap ? [0, 1].map((i) => (i ? 'H ' : 'W ') + oneWorldText('snap_dim', '{n} cells · {km} km', {
    n: snap.cells[i], km: snap.size_km[i].toFixed(1),
  })) : null;
  postToMap({ type: 'snapOverlay', snap, dims });
  refreshBboxSelectionInfo();
  const info = document.getElementById('bbox-snap-info');
  if (!info) return;
  info.style.display = snap ? '' : 'none';
  info.textContent = text;
}

let worldPath = "";

// Name of the world the last run made (or is making), as the label shows it.
// A pending custom name is drawn over it without replacing it.
let worldLabelName = "";

function setWorldNameLabel(text) {
  worldLabelName = text || "";
  renderWorldNameLabel();
}

function renderWorldNameLabel() {
  const label = document.getElementById('world-name-label');
  if (!label) return;
  const pending = pendingWorldName();
  const text = pending || worldLabelName;
  label.toggleAttribute('data-pending', !!pending);
  label.title = pending
    ? (window.localization && window.localization.placeholder_world_name) || 'Name of the next world'
    : '';
  if (text) {
    label.removeAttribute('data-placeholder');
    label.textContent = text;
  } else {
    label.setAttribute('data-placeholder', 'true');
    // Before the first localization pass there is nothing to look the text up
    // in, and that pass fills the placeholder itself. Looking it up here
    // instead fetched English, which arrived after the translation and
    // overwrote it.
    if (window.localization) {
      localizeElement(window.localization, { element: label }, 'no_world_generated_yet');
    }
  }
}

function basenameFromPath(p) {
  if (!p) return "";
  return p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "";
}

/* Custom world name (Java only, opt-in via Settings > Custom World Name) */

// Name for the next Java world, "" for the default "Arnis World N". The pencil
// only ever names the world the next Start creates, never one already on disk,
// and that world uses the name up. Only sent while the setting is on; the
// backend sanitizes it and appends " (2)" if the folder is taken.
let customWorldName = "";

function isCustomWorldNameFeatureEnabled() {
  const toggle = document.getElementById('custom-world-name-toggle');
  // Custom names are Java-only: creating a Bedrock/Luanti world never calls
  // gui_create_world, so offering the feature there would silently do
  // nothing.
  return !!(toggle && toggle.checked) && selectedWorldFormat === 'java';
}

// The name the next Start will ask for, if any.
function pendingWorldName() {
  if (isOneWorldEnabled() || !isCustomWorldNameFeatureEnabled()) return "";
  return customWorldName;
}

function canEditCustomWorldName() {
  // While a generation is in flight the next world's name is already taken
  // from here, so an edit would only look like it applied to the running one.
  return isCustomWorldNameFeatureEnabled() && generationButtonEnabled;
}

function updateWorldNamePreviewLabel() {
  if (isOneWorldEnabled()) {
    setWorldNameLabel(oneWorldDisplayName());
    return;
  }
  renderWorldNameLabel();
}

// Cancels any in-progress edit and shows/hides the pencil to match the
// current setting + world format. Safe to call anytime state that affects
// availability changes (toggle flipped, format switched, language changed).
function refreshWorldNameEditUI() {
  endWorldNameEdit();
  const editButton = document.getElementById('world-name-edit-button');
  if (editButton) {
    editButton.style.display = canEditCustomWorldName() ? '' : 'none';
    const title = isOneWorldEnabled() ? 'Choose the One World to extend or create' : 'Name the next world';
    editButton.title = title;
    editButton.setAttribute('aria-label', title);
  }
  updateWorldNamePreviewLabel();
}

// Marks the editor red once it is full. The input's own maxlength is what
// actually refuses further characters; this only makes that visible. The
// limit is read off that same attribute rather than duplicating the number
// here, so the colour can never disagree with what the field accepts.
function updateWorldNameLimitState() {
  const input = document.getElementById('world-name-input');
  if (!input) return;
  const limit = input.maxLength;
  if (limit > 0 && input.value.length >= limit) {
    input.setAttribute('data-at-limit', 'true');
  } else {
    input.removeAttribute('data-at-limit');
  }
}

function startWorldNameEdit(event) {
  if (event) {
    event.preventDefault();
    event.stopPropagation();
  }
  if (!canEditCustomWorldName()) return;

  const label = document.getElementById('world-name-label');
  const input = document.getElementById('world-name-input');
  const editButton = document.getElementById('world-name-edit-button');
  if (!label || !input) return;

  // Starts from the pending name, never from the last world's: that world
  // keeps its name, and asking for it again would only get "Name (2)".
  input.value = isOneWorldEnabled() ? oneWorldFolderName() : customWorldName;
  label.style.display = 'none';
  if (editButton) editButton.style.display = 'none';
  input.style.display = '';
  updateWorldNameLimitState();
  input.focus();
  input.select();
}

// Commits the pencil editor: remembers the name for the next generation, or
// clears it when left blank. Nothing on disk changes.
function commitWorldNameEdit() {
  const input = document.getElementById('world-name-input');
  if (!input) {
    endWorldNameEdit();
    return;
  }
  const newName = input.value.trim();

  // One World: the name picks the world to extend or create. Nothing is renamed.
  if (isOneWorldEnabled()) {
    setOneWorldName(newName === ONE_WORLD_DEFAULT_NAME ? '' : newName);
    endWorldNameEdit();
    refreshOneWorldState();
    return;
  }

  customWorldName = newName;
  endWorldNameEdit();
}

// Hiding the still-focused input fires a native blur (asynchronously, after
// the handler that hid it has returned), which would otherwise re-enter the
// blur handler below and re-run the edit we are already finishing: Escape
// would re-commit what it just discarded.
// endWorldNameEdit() sets this whenever it hides a focused input.
let suppressNextWorldNameBlur = false;

// Shared teardown for both commit and cancel: hides the input, restores the
// label (and the pencil, if the feature is still enabled), and refreshes
// what the label shows.
function endWorldNameEdit() {
  const label = document.getElementById('world-name-label');
  const input = document.getElementById('world-name-input');
  const editButton = document.getElementById('world-name-edit-button');
  if (!input || input.style.display === 'none') return;
  // Only when the input still holds focus: a teardown reached *from* the blur
  // handler must not arm this, or it would swallow the next real blur.
  if (document.activeElement === input) suppressNextWorldNameBlur = true;
  input.style.display = 'none';
  if (label) label.style.display = '';
  if (editButton && canEditCustomWorldName()) editButton.style.display = '';
  updateWorldNamePreviewLabel();
}

function initCustomWorldNameToggle() {
  const toggle = document.getElementById('custom-world-name-toggle');
  const editButton = document.getElementById('world-name-edit-button');
  const input = document.getElementById('world-name-input');
  if (!toggle) return;

  // Covers manual clicks and settings-store restoring/reverting the value
  // (both dispatch a real "change" event), as long as this listener is
  // attached before initSettingsStore() runs its restore().
  toggle.addEventListener('change', refreshWorldNameEditUI);

  if (editButton) {
    editButton.addEventListener('click', startWorldNameEdit);
    editButton.addEventListener('mousedown', (event) => event.stopPropagation());
  }

  if (input) {
    // The input lives inside .world-name-row, a sibling of #start-button
    // (not nested inside it) that has its own onclick="startGeneration()"
    // for clicks on the label/background; without this, placing the caret
    // would bubble up and trigger that too.
    input.addEventListener('click', (event) => event.stopPropagation());
    input.addEventListener('mousedown', (event) => event.stopPropagation());
    // "input" rather than "keydown": it also covers pasting, cutting and
    // undo, and fires after the value has actually changed.
    input.addEventListener('input', updateWorldNameLimitState);
    input.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') {
        event.preventDefault();
        commitWorldNameEdit();
      } else if (event.key === 'Escape') {
        // Discard: tear the editor down without committing.
        event.preventDefault();
        endWorldNameEdit();
      }
    });
    input.addEventListener('blur', () => {
      if (suppressNextWorldNameBlur) {
        suppressNextWorldNameBlur = false;
        return;
      }
      commitWorldNameEdit();
    });
  }

  refreshWorldNameEditUI();
}

/* One World (Java only, opt-in via Settings > One World) */

const ONE_WORLD_DEFAULT_NAME = "Arnis One World";
const ONE_WORLD_NAME_KEY = 'arnis-one-world-name';
// The user's own values of the settings a One World pins, put back when the
// mode goes off.
const ONE_WORLD_RESTORE_KEY = 'arnis-one-world-restore';
// Last answer from gui_one_world_info, null while the mode is off.
let oneWorldInfo = null;
let oneWorldPinned = false;
// Key the map overlays were last sent for.
let oneWorldOverlayKey = null;
// Whether the running (or last) generation was a One World run.
let lastRunOneWorld = false;
// The One World it went to: the named one, or a new one for a large selection.
let lastRunWorldName = '';
let oneWorldName = localStorage.getItem(ONE_WORLD_NAME_KEY) || '';

function isOneWorldAvailable() {
  return selectedWorldFormat === 'java' && selectedCelestialBody === 'earth';
}

function isOneWorldToggleOn() {
  const toggle = document.getElementById('one-world-toggle');
  return !!(toggle && toggle.checked);
}

// Luanti cannot hold a One World, so it is off while the toggle is.
function refreshLuantiAvailability() {
  const on = isOneWorldToggleOn();
  const luantiBtn = document.getElementById('format-luanti');
  if (luantiBtn) {
    luantiBtn.disabled = on;
    luantiBtn.title = on ? 'Luanti is not available in One World mode' : '';
  }
  setSettingsRowAvailable('enable-luanti-toggle', !on);
  if (on && selectedWorldFormat === 'luanti') setWorldFormat('java');
}

function isOneWorldEnabled() {
  const toggle = document.getElementById('one-world-toggle');
  return !!(toggle && toggle.checked) && isOneWorldAvailable();
}

// The name picks the world; it never renames one on disk.
function oneWorldFolderName() {
  if (isCustomWorldNameFeatureEnabled() && oneWorldName) return oneWorldName;
  return ONE_WORLD_DEFAULT_NAME;
}

function oneWorldDisplayName() {
  if (oneWorldInfo && oneWorldInfo.world_path) return basenameFromPath(oneWorldInfo.world_path);
  return oneWorldFolderName();
}

function setOneWorldName(name) {
  oneWorldName = name;
  if (name) localStorage.setItem(ONE_WORLD_NAME_KEY, name);
  else localStorage.removeItem(ONE_WORLD_NAME_KEY);
}

function oneWorldText(key, fallback, vars) {
  let text = (window.localization && window.localization[key]) || fallback;
  for (const k in (vars || {})) text = text.split('{' + k + '}').join(vars[k]);
  return text;
}

function setOneWorldStatus(text, tone) {
  const el = document.getElementById('one-world-status');
  if (!el) return;
  el.style.display = text ? '' : 'none';
  el.textContent = text || '';
  el.classList.toggle('is-warn', tone === 'warn');
  el.classList.toggle('is-error', tone === 'error');
}

function renderOneWorldStatus() {
  const info = oneWorldInfo;
  if (!isOneWorldEnabled() || !info) return;
  const name = oneWorldDisplayName();
  if (!savePath) {
    setOneWorldStatus(oneWorldText('one_world_no_save_path', 'Set the Minecraft saves folder in Settings first'), 'error');
    return;
  }
  if (info.foreign) {
    setOneWorldStatus(oneWorldText('one_world_foreign', 'Folder "{name}" exists but is not a One World', { name }), 'error');
    return;
  }
  if (!info.exists) {
    setOneWorldStatus(oneWorldText('one_world_will_create', 'One World "{name}" will be created', { name }));
    return;
  }
  let text = oneWorldText('one_world_areas', 'One World "{name}" \u00b7 Areas: {count}', { name, count: info.area_count });
  let tone = '';
  if (info.locked) {
    text += ' \u00b7 ' + oneWorldText('one_world_locked', 'open in Minecraft');
    tone = 'warn';
  }
  const mode = document.getElementById('generation-mode-select');
  const wantsTerrain = mode ? mode.value !== 'geo-only' : true;
  if (typeof info.terrain === 'boolean' && info.terrain !== wantsTerrain) {
    text += ' \u00b7 ' + oneWorldText(info.terrain ? 'one_world_needs_terrain' : 'one_world_needs_flat',
      info.terrain ? 'this world uses terrain' : 'this world is flat');
    tone = 'warn';
  }
  setOneWorldStatus(text, tone);
}

function setSettingsRowAvailable(inputId, available) {
  const input = document.getElementById(inputId);
  if (!input) return;
  input.disabled = !available;
  const row = input.closest('.settings-row');
  if (row) {
    row.classList.toggle('settings-row-unavailable', !available);
    row.toggleAttribute('inert', !available);
  }
}

function readOneWorldRestore() {
  try {
    return JSON.parse(localStorage.getItem(ONE_WORLD_RESTORE_KEY) || '{}') || {};
  } catch (_) {
    return {};
  }
}

function writeOneWorldRestore(record) {
  if (Object.keys(record).length) localStorage.setItem(ONE_WORLD_RESTORE_KEY, JSON.stringify(record));
  else localStorage.removeItem(ONE_WORLD_RESTORE_KEY);
}

function controlValue(el) {
  return el.type === 'checkbox' ? el.checked : parseFloat(el.value);
}

// Tells the store and every listener a control changed.
function fireInputChange(el) {
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

// A Browse button: the picker opens at `start()` and hands it back on cancel;
// anything else goes into the field.
function bindBrowse(buttonId, field, command, start, arg = 'current') {
  document.getElementById(buttonId).addEventListener('click', async () => {
    try {
      const current = start();
      const picked = await invoke(command, { [arg]: current });
      if (picked && picked !== current) {
        field.value = picked;
        fireInputChange(field);
      }
    } catch (error) {
      console.error(command + ' failed:', error);
    }
  });
}

function writeControl(el, value) {
  if (controlValue(el) === value) return;
  if (el.type === 'checkbox') el.checked = !!value;
  else el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

// Pins a control to the world's value, remembering the user's own once.
// `null` puts the user's value back.
function pinControl(id, value) {
  const el = document.getElementById(id);
  if (!el) return;
  const record = readOneWorldRestore();
  if (value === null) {
    if (id in record) {
      writeControl(el, record[id]);
      delete record[id];
      writeOneWorldRestore(record);
    }
    return;
  }
  if (!(id in record)) {
    record[id] = controlValue(el);
    writeOneWorldRestore(record);
  }
  writeControl(el, value);
}

function restoreNaturalRows() {
  setSettingsRowAvailable('scale-value-slider', selectedCelestialBody === 'earth');
  setSettingsRowAvailable('height-multiplier-slider', true);
  setSettingsRowAvailable('aws-only-elevation-toggle', selectedCelestialBody === 'earth');
  setSettingsRowAvailable('voxy-lod-toggle', true);
  setSettingsRowAvailable('disable-height-limit-toggle', true);
  refreshHeightLimitRow();
}

// Rotation stays 0, the Voxy cache off and the build height extended while
// the mode is on; scale, build height and elevation source follow the world
// once it exists.
function applyOneWorldPins(info) {
  oneWorldPinned = true;
  const rotationInput = document.getElementById('rotation-angle-input');
  if (rotationInput && parseFloat(rotationInput.value) !== 0 && window.updateRotation) window.updateRotation(0);
  setSettingsRowAvailable('rotation-angle-input', false);
  setSettingsRowAvailable('voxy-lod-toggle', false);
  postToMap({ type: 'setRotationLocked', locked: true });

  const exists = !!(info && info.exists);
  pinControl('scale-value-slider', exists && typeof info.scale === 'number' ? info.scale : null);
  pinControl('height-multiplier-slider',
    exists && typeof info.height_multiplier === 'number' ? info.height_multiplier : null);
  pinControl('disable-height-limit-toggle',
    exists && typeof info.disable_height_limit === 'boolean' ? info.disable_height_limit : true);
  restoreNaturalRows();
  setSettingsRowAvailable('voxy-lod-toggle', false);
  setSettingsRowAvailable('disable-height-limit-toggle', false);
  if (exists) {
    setSettingsRowAvailable('scale-value-slider', false);
    setSettingsRowAvailable('height-multiplier-slider', false);
    setSettingsRowAvailable('aws-only-elevation-toggle', false);
  }
}

function releaseOneWorldPins() {
  if (!oneWorldPinned) return;
  oneWorldPinned = false;
  pinControl('scale-value-slider', null);
  pinControl('height-multiplier-slider', null);
  pinControl('disable-height-limit-toggle', null);
  setSettingsRowAvailable('rotation-angle-input', true);
  restoreNaturalRows();
  postToMap({ type: 'setRotationLocked', locked: false });
}

function clearOneWorldOverlays() {
  if (oneWorldOverlayKey === null) return;
  oneWorldOverlayKey = null;
  postToMap({ type: 'oneWorldOverlays', areas: [] });
}

function isWorldNameEditing() {
  const input = document.getElementById('world-name-input');
  return !!(input && input.style.display !== 'none');
}

async function fetchOneWorldInfo() {
  return invoke('gui_one_world_info', { savePath: savePath, worldName: oneWorldFolderName() });
}

// Re-reads the world the next area joins. Call whenever something that picks
// the world changes: toggle, name, save path, format, body, window focus.
let oneWorldRefreshSeq = 0;
async function refreshOneWorldState() {
  const seq = ++oneWorldRefreshSeq;
  refreshLuantiAvailability();
  refreshAdvancedFeatures();
  setSettingsRowAvailable('one-world-toggle', isOneWorldAvailable());
  if (!isOneWorldEnabled()) {
    const wasOn = oneWorldInfo !== null;
    oneWorldInfo = null;
    releaseOneWorldPins();
    setOneWorldStatus('');
    clearOneWorldOverlays();
    if (wasOn) setWorldNameLabel(basenameFromPath(worldPath));
    if (!isWorldNameEditing()) refreshWorldNameEditUI();
    return;
  }
  if (!savePath) {
    oneWorldInfo = { exists: false, foreign: false };
    applyOneWorldPins(oneWorldInfo);
    renderOneWorldStatus();
    return;
  }
  let info;
  try {
    info = await fetchOneWorldInfo();
  } catch (error) {
    if (seq !== oneWorldRefreshSeq) return;
    console.error('Failed to read the One World:', error);
    oneWorldInfo = null;
    setOneWorldStatus(String(error), 'error');
    clearOneWorldOverlays();
    return;
  }
  if (seq !== oneWorldRefreshSeq) return;
  oneWorldInfo = info;
  applyOneWorldPins(info);
  if (!isWorldNameEditing()) refreshWorldNameEditUI();
  renderOneWorldStatus();

  if (!info.exists || info.foreign) {
    clearOneWorldOverlays();
    return;
  }
  const key = info.world_path + '|' + info.revision + '|' + info.area_count;
  if (key === oneWorldOverlayKey) return;
  try {
    const overlays = await invoke('gui_get_one_world_overlays', { worldPath: info.world_path });
    if (seq !== oneWorldRefreshSeq) return;
    oneWorldOverlayKey = key;
    postToMap({
      type: 'oneWorldOverlays',
      world_path: info.world_path,
      areas: (overlays && overlays.areas) || [],
      origin_lat: overlays ? overlays.origin_lat : 0,
      origin_lon: overlays ? overlays.origin_lon : 0,
      scale: overlays ? overlays.scale : 1
    });
  } catch (error) {
    console.error('Failed to load the One World overlays:', error);
  }
}

// Resolves when the user answers the overlap question.
function confirmOneWorldOverlap(count) {
  return new Promise((resolve) => {
    const modal = document.getElementById('one-world-confirm-modal');
    const text = document.getElementById('one-world-confirm-text');
    const ok = document.getElementById('one-world-confirm-ok');
    const cancel = document.getElementById('one-world-confirm-cancel');
    const close = document.getElementById('one-world-confirm-close');
    if (!modal || !text || !ok || !cancel) {
      resolve(true);
      return;
    }
    text.textContent = oneWorldText('one_world_confirm_text',
      '{count} chunks of this area already exist in the world. They are generated again, and anything built there in Minecraft is replaced.',
      { count });
    const finish = (answer) => {
      hideModal(modal);
      ok.removeEventListener('click', onOk);
      cancel.removeEventListener('click', onCancel);
      if (close) close.removeEventListener('click', onCancel);
      document.removeEventListener('keydown', onKey);
      resolve(answer);
    };
    const onOk = () => finish(true);
    const onCancel = () => finish(false);
    const onKey = (event) => { if (event.key === 'Escape') finish(false); };
    ok.addEventListener('click', onOk);
    cancel.addEventListener('click', onCancel);
    if (close) close.addEventListener('click', onCancel);
    document.addEventListener('keydown', onKey);
    showModal(modal);
    ok.focus();
  });
}

// Checks the world right before a run. Returns false when the run must not start.
async function prepareOneWorldRun(bbox, skipOverlap = false) {
  if (!savePath) {
    renderOneWorldStatus();
    return false;
  }
  let info;
  try {
    info = await fetchOneWorldInfo();
  } catch (error) {
    setOneWorldStatus(String(error), 'error');
    return false;
  }
  oneWorldInfo = info;
  applyOneWorldPins(info);
  renderOneWorldStatus();
  if (info.foreign) return false;
  if (info.locked) {
    setOneWorldStatus(oneWorldText('one_world_areas', 'One World "{name}" \u00b7 Areas: {count}',
      { name: oneWorldDisplayName(), count: info.area_count }) + ' \u00b7 ' +
      oneWorldText('one_world_locked', 'open in Minecraft'), 'error');
    return false;
  }
  if (!info.exists || skipOverlap) return true;
  let overlap = 0;
  try {
    overlap = await invoke('gui_one_world_overlap', {
      savePath: savePath, worldName: oneWorldFolderName(), bboxText: bbox
    });
  } catch (error) {
    setOneWorldStatus(String(error), 'error');
    return false;
  }
  return overlap > 0 ? confirmOneWorldOverlap(overlap) : true;
}

function postToMap(message) {
  const mapFrame = document.querySelector('.map-container');
  if (mapFrame && mapFrame.contentWindow) {
    mapFrame.contentWindow.postMessage(message, '*');
  }
}

function initOneWorld() {
  const toggle = document.getElementById('one-world-toggle');
  if (!toggle) return;
  // Also fires when the settings store restores the value.
  toggle.addEventListener('change', refreshOneWorldState);
  const mode = document.getElementById('generation-mode-select');
  if (mode) mode.addEventListener('change', renderOneWorldStatus);
  const nameToggle = document.getElementById('custom-world-name-toggle');
  if (nameToggle) nameToggle.addEventListener('change', () => { if (isOneWorldEnabled()) refreshOneWorldState(); });
  // Minecraft may have opened or closed the world meanwhile.
  window.addEventListener('focus', () => {
    if (isOneWorldEnabled() && generationButtonEnabled) refreshOneWorldState();
  });
  // A reloaded map iframe comes back without overlays.
  const mapFrame = getMapFrame();
  if (mapFrame) {
    mapFrame.addEventListener('load', () => {
      oneWorldOverlayKey = null;
      // The new map has no grid yet.
      snapPreviewKey = null;
      refreshSnapPreview();
      if (isOneWorldEnabled()) refreshOneWorldState();
    });
  }
}

/**
 * Handles world selection errors and displays appropriate messages
 * @param {number} errorCode - Error code from the backend
 */
function handleWorldSelectionError(errorCode) {
  const errorKeys = {
    1: "minecraft_directory_not_found",
    2: "world_in_use",
    3: "failed_to_create_world",
    4: "no_world_selected_error"
  };

  const errorKey = errorKeys[errorCode] || "unknown_error";
  const progressInfo = document.getElementById('progress-info');
  localizeElement(window.localization, { element: progressInfo }, errorKey);
  progressInfo.style.color = "#fa7878";
  worldPath = "";
  setWorldNameLabel("");
  console.error(errorCode);
}

let generationButtonEnabled = true;

// Pause, Stop and Restart, in the Start button's place while a run goes, and
// the run drawn on the map (bbox.js drawRunOverlay). `run` is null when idle.
// Pause holds a job's queue: running pieces finish, none starts until Resume.
// Stop kills what runs; a job keeps its finished pieces, so starting it again
// resumes. Restart is Stop, then Start: a job resumes unless "Start fresh"
// is chosen in its dialog; a run in one go always starts over.
let run = null; // { src, bbox, pieced, prewarm, worldName, paused, stopping, restart, retry }
// A cell's `s`: run, done, failed or stopped; the last two can be retried alone.
let runMap = null; // { src, units, bbox: [s, w, n, e], pct, state, pieces: { i: { b, s, f } } | null }
// The world a stopped or failed job of pieces was building, when One World was
// off and the job named a new one: its selection's next run goes there.
let resumeWorld = null; // { bbox, name }

function hasUnitRegionsFlag(flags) {
  return advancedFeatureArgs().flags.concat(flags || [])
    .some((f) => f.startsWith('--unit-regions') || f.startsWith('--one-world-workers'));
}

function bboxBounds(text) {
  const v = String(text || '').trim().split(/[,\s]+/).map(Number);
  if (v.length !== 4 || !v.every(isFinite)) return null;
  return [Math.min(v[0], v[2]), Math.min(v[1], v[3]), Math.max(v[0], v[2]), Math.max(v[1], v[3])];
}

// `idle`: no run going, so the map offers a failed or stopped cell's retry.
function postRunMap() {
  postToMap({ type: 'runOverlay', run: runMap && Object.assign({ idle: !run }, runMap) });
}

function unitRegionsValue() {
  return parseInt(document.getElementById('unit-regions-select').value, 10) || 4;
}

function startRunControls(opts) {
  run = Object.assign({ paused: false, stopping: false, restart: null, src: selectedBBox }, opts);
  // A retry keeps the other failed and stopped cells of the run before it.
  const kept = {};
  if (opts.retry && runMap && runMap.pieces && runMap.src === selectedBBox) {
    for (const k in runMap.pieces) {
      const c = runMap.pieces[k];
      if ((c.s === 'failed' || c.s === 'stopped') && !opts.retry.includes(+k)) kept[k] = c;
    }
  }
  runMap = opts.prewarm ? null
    : { src: selectedBBox, units: unitRegionsValue(), bbox: bboxBounds(opts.bbox), pct: 0, state: 'run', pieces: opts.pieced ? kept : null };
  postRunMap();
  syncRunControls();
}

// A job's `piece` record: its cell turns yellow while it builds, green when done.
function onRunPiece(p) {
  if (!runMap) return;
  if (!runMap.pieces) runMap.pieces = {};
  const cell = runMap.pieces[p.piece] || {};
  if (p.bounds) cell.b = p.bounds;
  if (p.state === 'done' || p.state === 'skipped') cell.s = 'done';
  else if (p.state === 'start' || p.state === 'retry') { cell.s = 'run'; cell.f = 0; }
  else if (p.state === 'progress') cell.f = p.fraction;
  else if (p.state === 'failed' || p.state === 'stopped') cell.s = p.state;
  else { delete runMap.pieces[p.piece]; postRunMap(); return; }
  runMap.pieces[p.piece] = cell;
  postRunMap();
}

// A run in one go fills its selection from the north as the bar moves.
function onRunPercent(pct) {
  if (!runMap || runMap.pieces || runMap.state !== 'run') return;
  if (Math.floor(pct) === Math.floor(runMap.pct)) return;
  runMap.pct = pct;
  postRunMap();
}

function endRunControls(message) {
  if (!run) return;
  const restart = run.restart;
  if (run.pieced && !isOneWorldEnabled()) {
    resumeWorld = message.startsWith('Done!') ? null : { bbox: run.src, name: run.worldName };
  }
  run = null;
  if (runMap) {
    if (message.startsWith('Done!')) runMap.state = 'done';
    else if (runMap.pieces) {
      // A piece the run left unfinished can be retried alone, as a stopped one.
      for (const k in runMap.pieces) if (runMap.pieces[k].s === 'run') runMap.pieces[k].s = 'stopped';
    } else runMap.state = 'stopped';
    postRunMap();
  }
  syncRunControls();
  if (restart && !message.startsWith('Error!')) {
    // After the listener has handed the Start guard back.
    setTimeout(() => restartRun(restart), 0);
  }
}

async function restartRun(how) {
  if (how.fresh) {
    try {
      await invoke('gui_forget_finished_pieces', {
        savePath, worldName: how.worldName, bboxText: how.bbox,
        unitRegions: parseInt(document.getElementById('unit-regions-select').value, 10) || 4,
      });
    } catch (error) {
      const info = document.getElementById('progress-info');
      info.textContent = 'Error! ' + error;
      info.style.color = '#fa7878';
      return;
    }
  }
  startGeneration({ restart: true });
}

function syncRunControls() {
  const t = oneWorldText;
  const on = !!run;
  const wrap = document.querySelector('.start-button-wrap');
  wrap.classList.toggle('is-running', on);
  document.getElementById('start-button').hidden = on;
  document.getElementById('run-controls').hidden = !on;
  const pause = document.getElementById('run-pause');
  const stop = document.getElementById('run-stop');
  const restart = document.getElementById('run-restart');
  const miniPause = document.getElementById('mini-pause');
  const miniStop = document.getElementById('mini-stop');
  miniPause.hidden = miniStop.hidden = !on;
  // How Start behaves means nothing while a run goes, and the buttons need the room.
  document.getElementById('offline-first-toggle').hidden = on || !extraFeaturesOn();
  syncRetryRow();
  if (!on) return;
  const canPause = run.pieced && !run.stopping;
  const pauseText = run.paused ? t('run_resume', 'Resume') : t('run_pause', 'Pause');
  const pauseTip = !run.pieced
    ? t('run_pause_single', 'Only a run in pieces can pause. This one builds in one go.')
    : run.paused ? t('run_resume_tip', 'Start the waiting pieces again.')
      : t('run_pause_tip', 'Let the running pieces finish, then hold the rest. Nothing is lost.');
  const stopText = run.stopping ? t('run_stopping', 'Stopping...') : t('run_stop', 'Stop');
  const stopTip = run.pieced || run.prewarm
    ? t('run_stop_tip', 'Stop now. Finished pieces are kept, so starting again resumes.')
    : t('run_stop_single_tip', 'Stop now. A run in one go starts over next time.');
  const restartTip = run.pieced
    ? t('run_restart_tip', 'Stop, then start again: resume, or build every piece again.')
    : t('run_restart_single_tip', 'Stop, then start this run again from the beginning.');
  for (const [btn, mini, icon, text, tip, enabled] of [
    [pause, miniPause, run.paused ? 'play' : 'pause', pauseText, pauseTip, canPause],
    [stop, miniStop, 'square', stopText, stopTip, !run.stopping],
    [restart, null, 'reset', t('run_restart', 'Restart'), restartTip, !run.stopping && !run.prewarm],
  ]) {
    btn.querySelector('use').setAttribute('href', '#i-' + icon);
    btn.querySelector('span').textContent = text;
    btn.title = tip;
    btn.disabled = !enabled;
    if (!mini) continue;
    mini.querySelector('use').setAttribute('href', '#i-' + icon);
    mini.title = tip;
    mini.setAttribute('aria-label', text);
    mini.disabled = !enabled;
  }
  pause.classList.toggle('is-active', run.paused);
  miniPause.classList.toggle('is-active', run.paused);
}

// "Retry failed (n)" under the buttons, once a run of pieces ended with failed ones.
function syncRetryRow() {
  const failed = run ? [] : retryableCells('failed');
  document.getElementById('run-retry-row').hidden = failed.length === 0;
  if (!failed.length) return;
  const retry = document.getElementById('run-retry');
  retry.querySelector('span').textContent = oneWorldText('run_retry_failed', 'Retry failed ({n})', { n: failed.length });
  retry.title = oneWorldText('run_retry_failed_tip', 'Build only the failed pieces again. Start builds every piece still missing.');
}

// The pieces of the last run, on this selection and grid, in state `state`
// ('failed', or 'failed' and 'stopped' when null).
function retryableCells(state) {
  if (!runMap || !runMap.pieces || runMap.src !== selectedBBox || runMap.units !== unitRegionsValue()) return [];
  return Object.keys(runMap.pieces).filter((k) => {
    const s = runMap.pieces[k].s;
    return state ? s === state : s === 'failed' || s === 'stopped';
  }).map(Number);
}

// Builds only `pieces` of the last run's job (gui_retry_pieces); its other
// missing pieces wait for Start. The coordinator's done files and leases are
// the same as a resume's, so the world ends up as one uninterrupted run's.
async function retryCells(pieces) {
  const can = retryableCells(null);
  pieces = pieces.filter((p) => can.includes(p));
  if (run || generationButtonEnabled === false || pieces.length === 0) return;
  await invoke('gui_retry_pieces', { pieces });
  await startGeneration({ restart: true, retry: pieces });
  // Refused before it started: the next run must not take it.
  if (!run) await invoke('gui_retry_pieces', { pieces: [] }).catch(() => {});
}

async function togglePause() {
  if (!run || !run.pieced || run.stopping) return;
  run.paused = !run.paused;
  syncRunControls();
  await invoke('gui_pause_generation', { paused: run.paused });
}

async function stopRun(restart = null) {
  if (!run || run.stopping) return;
  run.stopping = true;
  run.restart = restart;
  syncRunControls();
  const info = document.getElementById('progress-info');
  info.textContent = oneWorldText('run_stopping', 'Stopping...');
  info.style.color = '#ececec';
  await invoke('gui_stop_generation');
}

// A job asks whether to resume or start fresh; a run in one go just starts over.
function askRestart() {
  if (!run || run.stopping) return;
  const how = { bbox: run.bbox, worldName: run.worldName, fresh: false };
  if (!run.pieced) {
    stopRun(how);
    return;
  }
  const modal = document.getElementById('run-restart-modal');
  const finish = (choice) => {
    hideModal(modal);
    document.removeEventListener('keydown', onKey);
    if (choice && run) stopRun(Object.assign(how, { fresh: choice === 'fresh' }));
  };
  const onKey = (event) => { if (event.key === 'Escape') finish(null); };
  document.getElementById('run-restart-resume').onclick = () => finish('resume');
  document.getElementById('run-restart-fresh').onclick = () => finish('fresh');
  document.getElementById('run-restart-cancel').onclick = () => finish(null);
  document.getElementById('run-restart-close').onclick = () => finish(null);
  document.addEventListener('keydown', onKey);
  showModal(modal);
  document.getElementById('run-restart-resume').focus();
}

document.getElementById('run-pause').addEventListener('click', togglePause);
document.getElementById('run-stop').addEventListener('click', () => stopRun());
document.getElementById('run-restart').addEventListener('click', askRestart);
document.getElementById('mini-pause').addEventListener('click', togglePause);
document.getElementById('mini-stop').addEventListener('click', () => stopRun());
document.getElementById('run-retry').addEventListener('click', () => retryCells(retryableCells('failed')));

// Central setter so every place that toggles generation state also
// refreshes the world-name pencil (hidden/blocked while a generation, or a
// rename, is in flight - see canEditCustomWorldName()).
function setGenerationButtonEnabled(enabled) {
  generationButtonEnabled = enabled;
  refreshWorldNameEditUI();
  // The two jobs exclude each other in the backend, so the other one greys out.
  refreshPrecomputeButton();
}

/**
 * Initiates the world generation process
 * Validates required inputs and sends generation parameters to the backend
 * @returns {Promise<void>}
 */
// `prewarm` downloads what this run would read and builds nothing, so no
// world is created, checked or renamed for it.
async function startGeneration(options = {}) {
  const prewarm = options.prewarm === true;
  if (generationButtonEnabled === false) {
    return;
  }
  // "Download & bake missing data first": the download runs, and its Done!
  // starts this generation (setupProgressListener).
  if (!prewarm && !options.restart && !options.afterDownload && extraFeaturesOn() && offlineFirstOn()) {
    const need = offlineNeed();
    if (need && need.download + need.bake > 0) {
      generateAfterDownload = true;
      return downloadAndBakeMissing();
    }
  }
  // The backend refuses this too, but only after gui_create_world has already
  // made an empty world for a run that is not going to happen. Said here, the
  // world is never created and the user is told where the machine has gone.
  if (precomputeRunning) {
    const info = document.getElementById('progress-info');
    if (info) {
      info.textContent = "Waiting for the Mapillary precompute. Cancel it in Settings, or let it finish.";
      info.style.color = "#fecc44";
    }
    return;
  }
  // Claim the guard before the first await. gui_create_world and gui_start_generation are
  // both awaited round-trips, so leaving the claim until after them lets a second click
  // through and starts a parallel run against the same process-global world floor.
  setGenerationButtonEnabled(false);
  let started = false;

  try {
    if (!selectedBBox || selectedBBox == "0.000000 0.000000 0.000000 0.000000") {
      const bboxSelectionInfo = document.getElementById('bbox-selection-info');
      setBboxSelectionInfo(bboxSelectionInfo, "select_location_first", "#fa7878");
      return;
    }

    // Past every synchronous refusal, so from here the click is a real start.
    resetProgressUi(STARTING_MESSAGE);

    const runSelection = await runSelectionFor(selectedBBox);
    const runBBox = runSelection.bbox;
    const oneWorld = runSelection.oneWorld;
    // Without One World on, a run in pieces makes a new One World there.
    if (oneWorld && !isOneWorldEnabled() && !savePath) {
      handleWorldSelectionError(1);
      return;
    }
    if (isOneWorldEnabled() && !prewarm && !(await prepareOneWorldRun(runBBox, options.restart === true))) {
      const info = document.getElementById('progress-info');
      if (info && info.textContent === STARTING_MESSAGE) info.textContent = "";
      return;
    }
    lastRunOneWorld = oneWorld;
    lastRunWorldName = runSelection.worldName;

    // Auto-create world for Java format (a One World is resolved by the backend)
    if (selectedWorldFormat === 'java' && !oneWorld && !prewarm) {
      if (!savePath) {
        console.warn("Cannot create world: save path not set");
        return;
      }
      try {
        const requestedName = pendingWorldName() || null;
        const worldName = await invoke('gui_create_world', { savePath: savePath, worldName: requestedName });
        if (worldName) {
          worldPath = worldName;
          // Used up: the world after this one gets the default name again
          // unless the pencil names it too.
          if (requestedName) customWorldName = "";
          setWorldNameLabel(basenameFromPath(worldName));
        }
      } catch (error) {
        handleWorldSelectionError(error);
        return;
      }
    }

    // Clear any existing world preview since we're generating a new one.
    // A One World keeps its areas on the map; the new one joins them at the end.
    if (!oneWorld && !prewarm) notifyWorldChanged();
    if (oneWorld && !prewarm) setWorldNameLabel(runSelection.worldName);

    // Get the map iframe reference
    const mapFrame = document.querySelector('.map-container');
    // Get spawn point coordinates if marker exists
    let spawnPoint = null;
    if (mapFrame && mapFrame.contentWindow && mapFrame.contentWindow.getSpawnPointCoords) {
      const coords = mapFrame.contentWindow.getSpawnPointCoords();
      // Convert object format to tuple format if coordinates exist
      if (coords) {
        spawnPoint = [coords.lat, coords.lng];
      }
    }

    // Get generation mode from dropdown
    var generationMode = document.getElementById("generation-mode-select").value;
    var terrain = (generationMode === "geo-terrain" || generationMode === "terrain-only");
    var skipOsmObjects = (generationMode === "terrain-only");

    var interior = document.getElementById("interior-toggle").checked;
    var fill_ground = document.getElementById("fillground-toggle").checked;
    var caves = document.getElementById("caves-toggle").checked;
    var legacy_trees = document.getElementById("legacy-trees-toggle").checked;
    var canopy_height = document.getElementById("canopy-height-toggle").checked;
    var maxTreeSizeBtn = document.querySelector("#max-tree-size-group .segment.active");
    var maxTreeSize = maxTreeSizeBtn ? maxTreeSizeBtn.dataset.maxTreeSize : "giant";
    var overture = document.getElementById("overture-toggle").checked;
    var use_3d = document.getElementById("use-3d-toggle").checked;
    var heightLimitToggle = document.getElementById("disable-height-limit-toggle");
    // Disabled means unsupported for this body or format, so never send a stale tick.
    var disable_height_limit = !heightLimitToggle.disabled && heightLimitToggle.checked;
    var aws_only_elevation = document.getElementById("aws-only-elevation-toggle").checked;
    var bake_lighting = document.getElementById("bake-lighting-toggle").checked;
    var voxy_lod = document.getElementById("voxy-lod-toggle").checked;
    var dh_lod = document.getElementById("dh-lod-toggle").checked;
    var scale = parseFloat(document.getElementById("scale-value-slider").value);
    var heightMultiplier = parseFloat(document.getElementById("height-multiplier-slider").value) || 1;
    if (oneWorld && oneWorldInfo && oneWorldInfo.exists) {
      if (typeof oneWorldInfo.scale === 'number') scale = oneWorldInfo.scale;
      if (typeof oneWorldInfo.height_multiplier === 'number') heightMultiplier = oneWorldInfo.height_multiplier;
      if (typeof oneWorldInfo.disable_height_limit === 'boolean') disable_height_limit = oneWorldInfo.disable_height_limit;
    } else if (oneWorld) {
      disable_height_limit = true;
    }
    // var ground_level = parseInt(document.getElementById("ground-level").value, 10);
    // DEPRECATED: Ground level input removed from UI
    var ground_level = -62;

    // Validate ground_level
    ground_level = isNaN(ground_level) || ground_level < -62 ? -62 : ground_level;

    // Get telemetry consent (defaults to false if not set)
    const telemetryConsent = window.getTelemetryConsent ? window.getTelemetryConsent() : false;

    // Get rotation angle
    var rotationAngle = oneWorld ? 0 : (parseFloat(document.getElementById("rotation-angle-input").value) || 0);

    var gamemodeBtn = document.querySelector("#gamemode-group .segment.active");
    var gamemode = gamemodeBtn ? gamemodeBtn.dataset.gamemode : "creative";
    var worldTypeBtn = document.querySelector("#world-type-group .segment.active");
    var worldType = worldTypeBtn ? worldTypeBtn.dataset.worldType : "void";
    var mapItem = document.getElementById("map-item-toggle").checked;
    var signageBtn = document.querySelector("#signage-group .segment.active");
    var signage = signageBtn ? signageBtn.dataset.signage : "basic";
    // Clock minutes -> Minecraft ticks (tick 0 = 06:00; 24:00 wraps to 00:00)
    var clockMinutes = (parseInt(document.getElementById("world-time-slider").value, 10) || 0) % 1440;
    var worldTime = Math.round(((clockMinutes + 1440 - 360) % 1440) * (24000 / 1440));

    // Pass the selected options to the Rust backend
    await invoke("gui_start_generation", {
        bboxText: runBBox,
        selectedWorld: oneWorld ? savePath : worldPath,
        bedrockSavePath: bedrockSavePath,
        luantiSavePath: luantiSavePath,
        worldScale: scale,
        heightMultiplier: heightMultiplier,
        groundLevel: ground_level,
        terrainEnabled: terrain,
        skipOsmObjects: skipOsmObjects,
        interiorEnabled: interior,
        fillgroundEnabled: fill_ground,
        cavesEnabled: caves,
        legacyTreesEnabled: legacy_trees,
        maxTreeSize: maxTreeSize,
        canopyHeightEnabled: canopy_height,
        overtureEnabled: overture,
        use3dEnabled: use_3d,
        disableHeightLimit: disable_height_limit,
        awsOnlyElevation: aws_only_elevation,
        bakeLightingEnabled: bake_lighting,
        voxyLodEnabled: voxy_lod,
        dhLodEnabled: dh_lod,
        isNewWorld: !prewarm,
        spawnPoint: spawnPoint,
        telemetryConsent: telemetryConsent || false,
        worldFormat: getEffectiveWorldFormat(),
        rotationAngle: rotationAngle,
        gamemode: gamemode,
        worldTime: worldTime,
        worldType: worldType,
        mapItem: mapItem,
        signage: signage,
        mapillaryToken: getMapillaryToken(),
        facadesEnabled: getFacadesEnabled(),
        facadeMode: getEffectiveFacadeMode(),
        buildingFacadesEnabled: getBuildingFacadesEnabled(),
        facadeDetail: getFacadeDetail(),
        celestialBodyName: selectedCelestialBody,
        oneWorld: oneWorld,
        oneWorldName: oneWorld ? runSelection.worldName : "",
        // A download refuses --offline, which only reads what it fetches.
        // A download runs on the Bake CPU Usage.
        flags: (prewarm
          ? bakeFlags(advancedFeatureArgs().flags.filter((f) => f !== '--offline')).concat('--prewarm')
          : advancedFeatureArgs().flags).concat(runSelection.flags)
    });

    console.log("Generation process started.");
    startRunControls({
      bbox: runBBox,
      pieced: !prewarm && runSelection.oneWorld && hasUnitRegionsFlag(runSelection.flags),
      prewarm,
      worldName: runSelection.worldName,
      retry: options.retry || null,
    });
    setEtaSignageExpected(signage !== "none" && getEffectiveWorldFormat() === "java");
    resetEta();
    started = true;
    window.arnisPreview3D?.setGenerationRunning(true);
  } catch (error) {
    console.error("Error starting generation:", error);
  } finally {
    // Hand the guard back unless a run actually started; once it has, the Done!/Error!
    // progress message releases it instead.
    if (!started) {
      // Some abort paths say why (handleWorldSelectionError writes here); the
      // rest would leave our placeholder claiming a run that never began, so
      // clear it only if nothing else has taken the line since.
      const info = document.getElementById('progress-info');
      if (info && info.textContent === STARTING_MESSAGE) info.textContent = "";
      runStats = null;
      renderRunStats();
      setGenerationButtonEnabled(true);
      window.arnisPreview3D?.setGenerationRunning(false);
    }
    refreshPrecomputeButton();
  }
}

// World preview overlay state
let worldPreviewEnabled = false;
let currentWorldMapData = null;

/**
 * Notifies the map iframe that world preview data is ready
 * Called when the backend emits the map-preview-ready event
 */
async function showWorldPreviewButton() {
  // A One World shows every area at once, reloaded when the run reports Done.
  if (lastRunOneWorld) return;
  // Try to load the world map data
  await loadWorldMapData();

  if (currentWorldMapData) {
    // Send data to the map iframe
    const mapFrame = document.querySelector('.map-container');
    if (mapFrame && mapFrame.contentWindow) {
      mapFrame.contentWindow.postMessage({
        type: 'worldPreviewReady',
        data: currentWorldMapData
      }, '*');
      console.log("World preview data sent to map iframe");
    }
  } else {
    console.warn("Map data not available yet");
  }
}

/**
 * Notifies the map iframe that the world has changed (reset preview)
 */
function notifyWorldChanged() {
  currentWorldMapData = null;
  const mapFrame = document.querySelector('.map-container');
  if (mapFrame && mapFrame.contentWindow) {
    mapFrame.contentWindow.postMessage({
      type: 'worldChanged'
    }, '*');
  }
}

/**
 * Loads the world map data from the backend
 */
async function loadWorldMapData() {
  try {
    const mapData = await invoke('gui_get_world_map_data', { worldPath: worldPath });
    if (mapData) {
      currentWorldMapData = mapData;
      console.log("World map data loaded successfully");
    }
  } catch (error) {
    console.error("Failed to load world map data:", error);
  }
}
