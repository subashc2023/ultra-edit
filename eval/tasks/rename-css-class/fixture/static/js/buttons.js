// Helpers shared by every page that uses the button classes from
// static/css/buttons.css. See docs/style-guide.md for when to use each kind
// of button.

const BUSY_CLASS = "is-busy";

/**
 * Show that a button is waiting for a request.
 *
 * A busy button is disabled and its label is replaced by busyLabel. Calling
 * setBusy(button, false) restores the label it had before.
 */
export function setBusy(button, busy, busyLabel = "Working...") {
  if (busy && button.dataset.idleLabel === undefined) {
    button.dataset.idleLabel = button.textContent;
    button.textContent = busyLabel;
  } else if (!busy && button.dataset.idleLabel !== undefined) {
    button.textContent = button.dataset.idleLabel;
    delete button.dataset.idleLabel;
  }
  button.disabled = busy;
  button.classList.toggle(BUSY_CLASS, busy);
}

/**
 * Make button the primary button of its group.
 *
 * A group shows at most one primary button, so any other primary button in
 * the same toolbar or form-actions row is demoted to an outline button.
 */
export function promote(button) {
  const group = button.closest(".toolbar, .form-actions");
  if (group) {
    for (const other of group.querySelectorAll(".btn-primary")) {
      if (other !== button) {
        demote(other);
      }
    }
  }
  button.classList.remove("btn-primary-outline");
  button.classList.add("btn-primary");
}

/** Turn a primary button back into an outline button. */
export function demote(button) {
  button.classList.replace("btn-primary", "btn-primary-outline");
}

/** True if button is currently styled as the primary button. */
export function isPrimary(button) {
  return button.classList.contains("btn-primary");
}

/**
 * Keep every primary button inside root busy until promise settles, and
 * return what it resolves to. Outline and link buttons stay usable, so the
 * reader can still leave the page.
 */
export async function whilePending(root, promise) {
  const buttons = [...root.querySelectorAll("button.btn-primary")];
  for (const button of buttons) {
    setBusy(button, true);
  }
  try {
    return await promise;
  } finally {
    for (const button of buttons) {
      setBusy(button, false);
    }
  }
}
