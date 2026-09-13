(() => {
  "use strict";

  const POLL_MS = 5000;
  const RETRY_MS = 10000;
  const reader = document.querySelector("[data-live-reader]");
  if (!reader) return;

  const toggle = reader.querySelector("[data-live-toggle]");
  const connection = reader.querySelector("[data-live-connection]");
  const lastImport = reader.querySelector("[data-live-last-import]");
  const observed = reader.querySelector("[data-live-observed]");
  const sourceState = reader.querySelector("[data-live-source-state]");
  const projection = reader.querySelector("[data-live-projection]");
  const storageKey = reader.dataset.liveStorageKey;
  let timer;

  const storedEnabled = () => {
    try {
      return sessionStorage.getItem(storageKey) === "enabled";
    } catch (_) {
      return false;
    }
  };

  const storeEnabled = (enabled) => {
    try {
      if (enabled) sessionStorage.setItem(storageKey, "enabled");
      else sessionStorage.removeItem(storageKey);
    } catch (_) {
      // Private browsing policies can disable storage; the current toggle still works.
    }
  };

  const schedule = (delay) => {
    clearTimeout(timer);
    if (toggle.checked) timer = setTimeout(poll, delay);
  };

  const updateStatus = (status) => {
    connection.textContent = status.connection;
    connection.dataset.state = status.connection;
    lastImport.textContent = status.last_successful_import || "Unavailable";
    observed.textContent = status.source_observed_at || "Unavailable";
    sourceState.textContent = status.source_state;
    projection.textContent = status.source_projection_status;
  };

  const readingPosition = (region) => {
    const anchors = Array.from(region.querySelectorAll(".timeline-anchor[id]"));
    const anchor = anchors.filter((item) => item.getBoundingClientRect().top <= 16).at(-1)
      || anchors[0];
    return {
      id: anchor ? anchor.id : null,
      top: anchor ? anchor.getBoundingClientRect().top : null,
      scrollY: window.scrollY,
    };
  };

  const detailKey = (detail) => {
    const anchor = detail.previousElementSibling;
    return anchor && anchor.classList.contains("timeline-anchor") ? anchor.id : null;
  };

  const replaceTranscript = async () => {
    const current = document.querySelector("[data-live-content]");
    if (!current) throw new Error("live transcript region is unavailable");
    const openStates = new Map(
      Array.from(current.querySelectorAll("details.message"))
        .map((item) => [detailKey(item), item.open])
        .filter(([key]) => key),
    );
    const position = readingPosition(current);
    const response = await fetch(window.location.href, {
      cache: "no-store",
      headers: { Accept: "text/html" },
    });
    if (!response.ok) throw new Error("transcript refresh failed");
    const parsed = new DOMParser().parseFromString(await response.text(), "text/html");
    const replacement = parsed.querySelector("[data-live-content]");
    if (!replacement) throw new Error("replacement transcript region is unavailable");
    current.replaceChildren(...Array.from(replacement.childNodes));
    for (const detail of current.querySelectorAll("details.message")) {
      const key = detailKey(detail);
      if (openStates.has(key)) {
        detail.open = openStates.get(key);
      }
    }
    const restored = position.id && document.getElementById(position.id);
    if (restored && position.top !== null) {
      window.scrollBy(0, restored.getBoundingClientRect().top - position.top);
    } else {
      window.scrollTo(0, position.scrollY);
    }
  };

  const poll = async () => {
    if (!toggle.checked) return;
    try {
      const response = await fetch(reader.dataset.liveStatusUrl, {
        cache: "no-store",
        headers: { Accept: "application/json" },
      });
      if (!response.ok) throw new Error("status probe failed");
      const status = await response.json();
      updateStatus(status);
      if (String(status.revision) !== reader.dataset.liveRevision) {
        await replaceTranscript();
        reader.dataset.liveRevision = String(status.revision);
      }
      schedule(document.visibilityState === "hidden" ? RETRY_MS : POLL_MS);
    } catch (_) {
      connection.textContent = "disconnected";
      connection.dataset.state = "disconnected";
      schedule(RETRY_MS);
    }
  };

  toggle.checked = storedEnabled();
  toggle.addEventListener("change", () => {
    storeEnabled(toggle.checked);
    connection.textContent = toggle.checked ? "connecting" : "paused";
    connection.dataset.state = toggle.checked ? "connecting" : "paused";
    if (toggle.checked) poll();
    else clearTimeout(timer);
  });
  document.addEventListener("visibilitychange", () => {
    if (toggle.checked && document.visibilityState === "visible") schedule(0);
  });
  if (toggle.checked) poll();
})();
