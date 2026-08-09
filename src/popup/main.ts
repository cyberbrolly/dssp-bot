import { MessageBus } from "../core/infrastructure/messaging/MessageBus";

const messageBus = new MessageBus();

const app = document.getElementById("app");

if (!app) {
  throw new Error("Popup root element not found.");
}

app.innerHTML = `
  <h1>DSSP Logger</h1>

  <p id="status">Ready</p>

  <button id="start">Start</button>
  <button id="pause">Pause</button>
  <button id="stop">Stop</button>
`;

const status = document.getElementById("status");

document.getElementById("start")?.addEventListener("click", async () => {
  const response = await messageBus.send({
    type: "START_AUTOMATION",
  });

  if (status) {
    status.textContent = response.success
      ? "Automation started"
      : `Error: ${response.error}`;
  }
});

document.getElementById("pause")?.addEventListener("click", async () => {
  const response = await messageBus.send({
    type: "PAUSE_AUTOMATION",
  });

  if (status) {
    status.textContent = response.success
      ? "Automation paused"
      : `Error: ${response.error}`;
  }
});

document.getElementById("stop")?.addEventListener("click", async () => {
  const response = await messageBus.send({
    type: "STOP_AUTOMATION",
  });

  if (status) {
    status.textContent = response.success
      ? "Automation stopped"
      : `Error: ${response.error}`;
  }
});