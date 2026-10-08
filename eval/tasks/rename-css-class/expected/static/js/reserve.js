// Reservation form on templates/reserve.html.
//
// The primary button stays disabled until a pickup branch is chosen. The form
// is then sent with fetch, so a successful reservation is shown in place
// without reloading the page.

import { promote, whilePending } from "./buttons.js";

export function initReserveForm(form) {
  const btnPrimary = form.querySelector(".form-actions .btn-accent");
  const backLink = form.querySelector(".form-actions .btn-primary-outline");
  const help = document.getElementById("btn-primary-help");
  const branch = form.elements.namedItem("branch");
  const label = form.getAttribute("data-btn-primary-label");

  function update() {
    const chosen = branch.value !== "";
    btnPrimary.disabled = !chosen;
    btnPrimary.textContent = chosen ? label : "Choose a branch first";
    help.hidden = !chosen;
  }

  branch.addEventListener("change", update);
  update();

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    let response = null;
    try {
      response = await whilePending(form, fetch(form.action, {
        method: "POST",
        body: new FormData(form),
      }));
    } catch {
      // A network failure is reported like a refused request below.
    }
    if (response && response.status === 409) {
      // The reader already has this book on hold, so the useful next step is
      // going back to the results, not trying again.
      help.textContent = "You already have a copy of this book on hold.";
      help.hidden = false;
      btnPrimary.disabled = true;
      promote(backLink);
      return;
    }
    if (!response || !response.ok) {
      help.textContent = "Sorry, the reservation did not go through. Please try again.";
      help.hidden = false;
      return;
    }
    showDone(form, branch.selectedOptions[0].textContent);
  });
}

function showDone(form, branchName) {
  const done = document.querySelector(".reserve-done");
  done.querySelector(".reserve-done__branch").textContent = branchName;
  form.hidden = true;
  done.hidden = false;
  // Move focus to the next action so keyboard users are not left on a
  // hidden form.
  done.querySelector("a.btn-accent").focus();
}

const form = document.querySelector("form.reserve-form");
if (form) {
  initReserveForm(form);
}
