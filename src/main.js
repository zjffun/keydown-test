const { listen } = window.__TAURI__.event;

let timerInterval = null;
let remainingSeconds = 180;

const statusEl = document.getElementById("status");
const timerEl = document.getElementById("timer");

function formatTime(seconds) {
  const m = String(Math.floor(seconds / 60)).padStart(2, "0");
  const s = String(seconds % 60).padStart(2, "0");
  return `${m}:${s}`;
}

function startCountdown() {
  // Reset if already done or running
  if (timerInterval) {
    clearInterval(timerInterval);
  }
  remainingSeconds = 180;
  timerEl.classList.remove("done");
  timerEl.classList.add("active");
  statusEl.textContent = "倒计时进行中…";
  statusEl.classList.add("active");
  timerEl.textContent = formatTime(remainingSeconds);

  timerInterval = setInterval(() => {
    remainingSeconds--;
    timerEl.textContent = formatTime(remainingSeconds);

    if (remainingSeconds <= 0) {
      clearInterval(timerInterval);
      timerInterval = null;
      statusEl.textContent = "倒计时结束";
      timerEl.classList.remove("active");
      timerEl.classList.add("done");
    }
  }, 1000);
}

listen("f6-pressed", () => {
  startCountdown();
});
