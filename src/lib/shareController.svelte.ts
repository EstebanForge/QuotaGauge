const TOAST_MS = 1800;

export function createShareController() {
  let message = $state<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;

  function show(next: string) {
    message = next;
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => (message = null), TOAST_MS);
  }

  function dismiss() {
    if (timer) clearTimeout(timer);
    message = null;
  }

  return {
    get message() {
      return message;
    },
    show,
    dismiss,
  };
}
