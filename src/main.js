const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;

// ── Timer ──────────────────────────────────────────────

let timerInterval = null;
let remainingSeconds = 180;

const timerEl = document.getElementById("timer");

function formatTime(s) {
  return String(Math.floor(s / 60)).padStart(2, "0") + ":" + String(s % 60).padStart(2, "0");
}

function startCountdown() {
  if (timerInterval) clearInterval(timerInterval);
  remainingSeconds = 180;
  timerEl.classList.remove("done");
  document.body.classList.remove("timer-done");
  timerEl.classList.add("active");
  timerEl.textContent = formatTime(remainingSeconds);

  timerInterval = setInterval(() => {
    remainingSeconds--;
    timerEl.textContent = formatTime(remainingSeconds);
    if (remainingSeconds <= 0) {
      clearInterval(timerInterval);
      timerInterval = null;
      timerEl.classList.remove("active");
      timerEl.classList.add("done");
      document.body.classList.add("timer-done");
    }
  }, 1000);
}

listen("f6-pressed", () => {
  startCountdown();
});

// ── Tab capture (10s session window) ───────────────────

const SESSION_MS = 10000;
let tabs = [];
let sessionStart = null;

const tabListEl = document.getElementById("tab-list");
const errorEl = document.getElementById("error-msg");

function renderTabs() {
  tabListEl.innerHTML = "";
  tabs.forEach((tab, i) => {
    const item = document.createElement("div");
    item.className = "tab-item";

    const label = document.createElement("div");
    label.className = "tab-label";
    label.textContent = `Tab ${i + 1}`;

    const img = document.createElement("img");
    img.className = "tab-img";
    img.src = tab.avatar_image;

    item.appendChild(label);
    item.appendChild(img);
    tabListEl.appendChild(item);
  });
}

listen("tab-captured", (event) => {
  const now = Date.now();
  errorEl.textContent = "";
  if (sessionStart === null || (now - sessionStart) > SESSION_MS) {
    tabs = [];
    sessionStart = now;
  }
  tabs.push(event.payload);
  renderTabs();
});

listen("capture-error", (event) => {
  errorEl.textContent = event.payload.message;
});

// ── Region selection ───────────────────────────────────

const selectBtn = document.getElementById("select-region-btn");
const regionStatus = document.getElementById("region-status");
const overlay = document.getElementById("select-overlay");
const canvas = document.getElementById("select-canvas");
const ctx = canvas.getContext("2d");

// Real screen dimensions (from backend)
let screenW = 0;
let screenH = 0;

// Restore saved region from localStorage
const saved = localStorage.getItem("captureRegion");
if (saved) {
  try {
    const r = JSON.parse(saved);
    regionStatus.textContent = `已设置: ${r.w}×${r.h} @ (${r.x}, ${r.y})`;
    regionStatus.className = "region-status ok";
    // Send to backend on startup
    invoke("save_region", { region: r }).catch(() => {});
  } catch (_) {}
}

selectBtn.addEventListener("click", async () => {
  selectBtn.disabled = true;
  regionStatus.textContent = "截图中…";

  try {
    const result = await invoke("take_screenshot");
    screenW = result.width;
    screenH = result.height;
    await showSelectionOverlay(result.image);
  } catch (e) {
    regionStatus.textContent = "截图失败: " + e;
    regionStatus.className = "region-status err";
  } finally {
    selectBtn.disabled = false;
  }
});

function showSelectionOverlay(dataUrl) {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      // Canvas fills the window; image is drawn scaled to fit
      canvas.width = window.innerWidth;
      canvas.height = window.innerHeight;
      overlay.style.display = "flex";

      // Scale factor: image pixels → canvas pixels
      const scaleX = canvas.width / img.width;
      const scaleY = canvas.height / img.height;
      const scale = Math.min(scaleX, scaleY);
      const drawW = img.width * scale;
      const drawH = img.height * scale;
      const offX = (canvas.width - drawW) / 2;
      const offY = (canvas.height - drawH) / 2;

      function drawBase() {
        ctx.clearRect(0, 0, canvas.width, canvas.height);
        ctx.fillStyle = "rgba(0,0,0,0.6)";
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(img, offX, offY, drawW, drawH);
      }

      drawBase();

      let dragging = false;
      let startX = 0, startY = 0;

      function onDown(e) {
        dragging = true;
        startX = e.clientX;
        startY = e.clientY;
      }

      function onMove(e) {
        if (!dragging) return;
        drawBase();

        const x = Math.min(startX, e.clientX);
        const y = Math.min(startY, e.clientY);
        const w = Math.abs(e.clientX - startX);
        const h = Math.abs(e.clientY - startY);

        // Dim outside selection
        ctx.fillStyle = "rgba(0,0,0,0.5)";
        ctx.fillRect(0, 0, canvas.width, y);
        ctx.fillRect(0, y, x, h);
        ctx.fillRect(x + w, y, canvas.width - x - w, h);
        ctx.fillRect(0, y + h, canvas.width, canvas.height - y - h);

        // Selection border
        ctx.strokeStyle = "#64ffda";
        ctx.lineWidth = 2;
        ctx.strokeRect(x, y, w, h);
      }

      function onUp(e) {
        if (!dragging) return;
        dragging = false;
        cleanup();

        const x = Math.min(startX, e.clientX);
        const y = Math.min(startY, e.clientY);
        const w = Math.abs(e.clientX - startX);
        const h = Math.abs(e.clientY - startY);

        if (w < 10 || h < 10) {
          overlay.style.display = "none";
          regionStatus.textContent = "选区太小，请重新框选";
          regionStatus.className = "region-status err";
          resolve();
          return;
        }

        // Convert canvas coords → real screen coords
        // The screenshot is 2× downscaled, so img dimensions = screen/2
        // Canvas coords → img coords → screen coords
        const imgX = (x - offX) / scale;
        const imgY = (y - offY) / scale;
        const imgW = w / scale;
        const imgH = h / scale;

        // img is at half resolution, so multiply by 2 to get real screen coords
        const region = {
          x: Math.round(imgX * 2),
          y: Math.round(imgY * 2),
          w: Math.round(imgW * 2),
          h: Math.round(imgH * 2),
        };

        // Clamp
        region.x = Math.max(0, region.x);
        region.y = Math.max(0, region.y);
        region.w = Math.min(region.w, screenW - region.x);
        region.h = Math.min(region.h, screenH - region.y);

        overlay.style.display = "none";

        // Save to backend and localStorage
        invoke("save_region", { region })
          .then(() => {
            localStorage.setItem("captureRegion", JSON.stringify(region));
            regionStatus.textContent = `已设置: ${region.w}×${region.h} @ (${region.x}, ${region.y})`;
            regionStatus.className = "region-status ok";
          })
          .catch((err) => {
            regionStatus.textContent = "保存失败: " + err;
            regionStatus.className = "region-status err";
          });

        resolve();
      }

      function onKeyDown(e) {
        if (e.key === "Escape") {
          dragging = false;
          cleanup();
          overlay.style.display = "none";
          resolve();
        }
      }

      function cleanup() {
        canvas.removeEventListener("mousedown", onDown);
        canvas.removeEventListener("mousemove", onMove);
        canvas.removeEventListener("mouseup", onUp);
        document.removeEventListener("keydown", onKeyDown);
      }

      canvas.addEventListener("mousedown", onDown);
      canvas.addEventListener("mousemove", onMove);
      canvas.addEventListener("mouseup", onUp);
      document.addEventListener("keydown", onKeyDown);
    };
    img.src = dataUrl;
  });
}
