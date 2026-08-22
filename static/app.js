"use strict";

// Write key lives only in this browser. Used for create-device + (shown in) quickstart.
const KEY_STORE = "sensordash_write_key";
const getKey = () => localStorage.getItem(KEY_STORE) || "";
const setKey = (v) => localStorage.setItem(KEY_STORE, v);

function timeAgo(ts) {
  if (!ts) return "no data yet";
  const s = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

// ---------------- Homepage ----------------

async function initIndex(devicesEl) {
  const form = document.getElementById("create-form");
  const nameEl = document.getElementById("device-name");
  const keyEl = document.getElementById("write-key");
  const msgEl = document.getElementById("create-msg");

  keyEl.value = getKey();
  keyEl.addEventListener("input", () => setKey(keyEl.value.trim()));

  const showMsg = (text, kind) => {
    msgEl.textContent = text;
    msgEl.className = "msg " + (kind || "");
    msgEl.hidden = false;
  };

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const name = nameEl.value.trim();
    const key = keyEl.value.trim();
    if (!key) {
      showMsg("Enter your write key first (expand “Write key”).", "error");
      return;
    }
    try {
      const res = await fetch("/api/devices", {
        method: "POST",
        headers: { "content-type": "application/json", "x-api-key": key },
        body: JSON.stringify({ name }),
      });
      if (res.ok) {
        location.href = "/device/" + encodeURIComponent(name);
      } else {
        showMsg(await res.text(), "error");
      }
    } catch (err) {
      showMsg("Network error: " + err, "error");
    }
  });

  await loadDevices(devicesEl);
}

async function loadDevices(el) {
  try {
    const res = await fetch("/api/devices");
    const { devices } = await res.json();
    if (!devices.length) {
      el.innerHTML = `<p class="muted empty-note">No devices yet. Create one below.</p>`;
      return;
    }
    el.innerHTML = "";
    for (const d of devices) {
      const a = document.createElement("a");
      a.className = "device-tile";
      a.href = "/device/" + encodeURIComponent(d.name);
      a.setAttribute("role", "button");
      a.classList.add("outline", "secondary");

      const h3 = document.createElement("h3");
      h3.textContent = d.name;
      const meta = document.createElement("p");
      meta.className = "meta";
      meta.textContent =
        `${d.sensor_count} sensor${d.sensor_count === 1 ? "" : "s"} · ${timeAgo(d.last_seen)}`;

      const article = document.createElement("article");
      article.replaceChildren(h3, meta);
      a.replaceChildren(article);
      el.appendChild(a);
    }
  } catch (err) {
    const p = document.createElement("p");
    p.className = "msg error";
    p.textContent = "Failed to load devices: " + err;
    el.replaceChildren(p);
  }
}

// ---------------- Device page ----------------

const charts = new Map(); // sensor name -> { chart, valueEl }

async function initDevice() {
  const name = decodeURIComponent(location.pathname.split("/").pop() || "");
  document.getElementById("device-title").textContent = name;
  document.getElementById("device-title").classList.remove("muted");
  document.title = "SensorDash — " + name;

  // If the write key is already saved in this browser, drop it straight into the
  // command so it's copy-paste ready; otherwise leave a YOUR_KEY placeholder.
  const storedKey = getKey();
  const keyForCmd = storedKey || "YOUR_KEY";
  const curl =
    `curl -H "X-API-Key: ${keyForCmd}" -d "23.5" ${location.origin}/update_sensor/${encodeURIComponent(name)}/temperature`;
  document.getElementById("quickstart").textContent = curl;
  document.getElementById("quickstart2").textContent = curl;

  // No need to tell them to swap in a key when we've already filled it in.
  const replaceNote = document.getElementById("replace-note");
  if (replaceNote) replaceNote.hidden = !!storedKey;

  wireCopyButtons();

  // A write key in this browser unlocks deleting the device.
  const delBtn = document.getElementById("delete-device");
  if (delBtn && storedKey) {
    delBtn.hidden = false;
    delBtn.addEventListener("click", () => deleteDevice(name, storedKey));
  }

  await refreshDevice(name);
  setInterval(() => refreshDevice(name), 60000); // refresh once a minute
}

async function refreshDevice(name) {
  const notfound = document.getElementById("notfound");
  const empty = document.getElementById("empty");
  const howto = document.getElementById("howto");

  let data;
  try {
    const res = await fetch(`/api/devices/${encodeURIComponent(name)}/data`);
    if (res.status === 404) {
      // Unknown device (typo'd URL, or never created).
      document.getElementById("notfound-name").textContent = name;
      notfound.hidden = false;
      empty.hidden = true;
      howto.hidden = true;
      return;
    }
    if (!res.ok) return; // transient error — keep whatever's on screen
    data = await res.json();
  } catch {
    return;
  }

  const sensors = data.sensors || [];
  notfound.hidden = true;
  empty.hidden = sensors.length > 0;
  howto.hidden = sensors.length === 0;

  for (const s of sensors) {
    const xs = s.points.map((p) => p[0]);
    const ys = s.points.map((p) => p[1]);
    const latest = ys.length ? ys[ys.length - 1] : null;
    const lastTs = xs.length ? xs[xs.length - 1] : null;

    let entry = charts.get(s.name);
    if (!entry) {
      entry = createSensorCard(s.name);
      charts.set(s.name, entry);
    }
    entry.valueEl.textContent = latest === null ? "—" : fmt(latest);
    entry.timeEl.textContent = lastTs ? "updated " + timeAgo(lastTs) : "";
    entry.chart.setData([xs, ys]);
  }
}

function createSensorCard(name) {
  const container = document.getElementById("charts");
  const card = document.createElement("article");
  card.className = "sensor-card";

  const header = document.createElement("header");
  const nameEl = document.createElement("span");
  nameEl.className = "sensor-name";
  nameEl.textContent = name;
  const valueEl = document.createElement("span");
  valueEl.className = "value";
  valueEl.textContent = "—";
  header.append(nameEl, valueEl);

  const chartEl = document.createElement("div");
  chartEl.className = "chart";

  const timeEl = document.createElement("small");
  timeEl.className = "muted updated";

  card.append(header, chartEl, timeEl);
  container.appendChild(card);

  const chart = new uPlot(chartOpts(name, chartEl.clientWidth), [[], []], chartEl);
  // Keep the chart width in sync with its (responsive grid) container.
  new ResizeObserver(() => chart.setSize({ width: chartEl.clientWidth, height: 200 }))
    .observe(chartEl);

  return { chart, valueEl, timeEl };
}

function chartOpts(label, width) {
  const axisColor = "#9aa4b2";
  const gridColor = "#2a2f3a";
  return {
    width: width || 320,
    height: 200,
    scales: { x: { time: true } },
    legend: { show: false },
    cursor: { points: { size: 6 } },
    axes: [
      { stroke: axisColor, grid: { stroke: gridColor, width: 1 }, ticks: { stroke: gridColor } },
      { stroke: axisColor, grid: { stroke: gridColor, width: 1 }, ticks: { stroke: gridColor }, size: 50 },
    ],
    series: [
      {},
      {
        label,
        stroke: "#4f8cff",
        width: 2,
        fill: "rgba(79,140,255,0.12)",
        points: { show: false },
      },
    ],
  };
}

async function deleteDevice(name, key) {
  if (!confirm(`Delete device "${name}" and ALL of its sensor readings?\n\nThis cannot be undone.`)) {
    return;
  }
  try {
    const res = await fetch(`/api/devices/${encodeURIComponent(name)}`, {
      method: "DELETE",
      headers: { "x-api-key": key },
    });
    if (res.ok) {
      location.href = "/";
    } else {
      alert("Delete failed: " + (await res.text()));
    }
  } catch (err) {
    alert("Network error: " + err);
  }
}

// ---------------- copy-to-clipboard ----------------

// Copy text to the clipboard. Prefers the async Clipboard API (which requires a
// secure context — HTTPS or localhost) and falls back to execCommand, so this also
// works when the app is served over plain HTTP.
function copyText(text) {
  if (navigator.clipboard && window.isSecureContext) {
    return navigator.clipboard.writeText(text);
  }
  return new Promise((resolve, reject) => {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    ta.setSelectionRange(0, text.length);
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    document.body.removeChild(ta);
    ok ? resolve() : reject(new Error("copy failed"));
  });
}

function wireCopyButtons() {
  for (const btn of document.querySelectorAll("[data-copy]")) {
    btn.addEventListener("click", async () => {
      const target = document.getElementById(btn.dataset.copy);
      if (!target) return;
      const original = btn.textContent;
      try {
        await copyText(target.textContent);
        btn.textContent = "Copied!";
      } catch {
        btn.textContent = "Press ⌘/Ctrl+C";
      }
      setTimeout(() => (btn.textContent = original), 1500);
    });
  }
}

// ---------------- helpers ----------------

function fmt(n) {
  if (!isFinite(n)) return String(n);
  const r = Math.round(n * 1000) / 1000;
  return String(r);
}

// ---------------- boot ----------------

const devicesEl = document.getElementById("devices");
if (devicesEl) {
  initIndex(devicesEl);
} else if (document.body.dataset.page === "device") {
  initDevice();
}
