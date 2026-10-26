<script lang="ts">
  import Icon from './Icon.svelte';
  import type { UpdateController } from './updateController.svelte';
  import type { AppSettings } from './types';

  interface Props {
    updates: UpdateController;
    settings: AppSettings;
    onChange: (settings: AppSettings) => void;
  }
  let { updates, settings, onChange }: Props = $props();
</script>

{#if updates.status?.available && updates.status.version !== settings.dismissedUpdateVersion}
  <section class="hint-card update-banner" aria-label="Update Available">
    <span class="hint-card__icon"><Icon name="refresh" size={16} strokeWidth={2} /></span>
    <div>
      <strong>Update Available</strong>
      <span>QuotaGauge {updates.status.version} is ready to download.</span>
      {#if updates.status.body}<details class="update-notes">
          <summary>What’s new</summary>
          <p>{updates.status.body}</p>
        </details>{/if}
      {#if updates.installing && updates.progress}
        <div
          class="update-progress"
          role="progressbar"
          aria-label="Update download"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={updates.progress.phase === 'installing'
            ? 100
            : (updates.progress.percent ?? undefined)}
        >
          <span
            style:width={`${updates.progress.phase === 'installing' ? 100 : (updates.progress.percent ?? 8)}%`}
          ></span>
        </div>
        <small>
          {updates.progress.phase === 'installing'
            ? 'Installing update…'
            : updates.progress.phase === 'retrying'
              ? 'Download interrupted. Retrying…'
              : updates.progress.percent === null
                ? 'Downloading update…'
                : `Downloading update… ${updates.progress.percent}%`}
        </small>
      {/if}
      {#if updates.error}<div class="update-error" role="alert">
          <strong>{updates.error.message}</strong><small>{updates.error.action}</small>
        </div>{/if}
    </div>
    <div class="update-actions">
      <button
        type="button"
        class="update-primary-action"
        onclick={updates.status.installable
          ? () => updates.install()
          : () => updates.openDownloadPage()}
        disabled={updates.installing}
        >{updates.status.installable
          ? updates.installing
            ? 'Updating…'
            : updates.error?.retryable
              ? 'Try Again'
              : 'Install Update'
          : 'Download from GitHub'}</button
      >
      {#if updates.status.installable && !updates.installing}
        <button
          type="button"
          class="update-release-action"
          onclick={() => updates.openDownloadPage()}>View Release</button
        >
      {/if}
    </div>
    <button
      class="hint-card__dismiss"
      type="button"
      aria-label="Dismiss"
      onclick={() =>
        onChange({
          ...settings,
          dismissedUpdateVersion: updates.status?.version ?? null,
        })}><Icon name="close" size={10} strokeWidth={2.2} /></button
    >
  </section>
{/if}

<style>
  .hint-card {
    position: relative;
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px 10px;
    margin-bottom: 14px;
    padding: 11px 32px 11px 12px;
    border-radius: 12px;
    background: var(--card);
  }

  .hint-card__icon {
    display: grid;
    width: 22px;
    height: 22px;
    grid-row: 1 / span 2;
    color: var(--meter-fill);
    place-items: center;
  }

  .hint-card > div {
    display: flex;
    min-width: 0;
    flex-direction: column;
    gap: 2px;
  }

  .hint-card strong {
    font-size: 12px;
  }

  .hint-card span {
    color: var(--secondary);
    font-size: 10px;
    line-height: 13px;
  }

  .update-actions button {
    width: fit-content;
    padding: 4px 8px;
    border: 0;
    border-radius: 6px;
    color: var(--text);
    background: var(--button-hover);
    font-size: 10px;
  }

  .hint-card > .update-actions {
    display: flex;
    grid-column: 2;
    align-items: center;
    flex-direction: row;
    gap: 6px;
    margin-top: 5px;
  }

  .update-actions .update-primary-action {
    color: white;
    background: var(--meter-fill);
    font-weight: 600;
  }

  .update-actions .update-release-action {
    color: var(--secondary);
    background: transparent;
  }

  .update-actions .update-release-action:hover,
  .update-actions .update-release-action:focus-visible {
    color: var(--text);
    background: var(--button-hover);
  }

  .hint-card__dismiss {
    position: absolute;
    top: 5px;
    right: 6px;
    display: grid;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    color: var(--secondary);
    background: transparent;
    cursor: pointer;
    place-items: center;
  }

  .hint-card__dismiss:hover,
  .hint-card__dismiss:focus-visible {
    outline: none;
    color: var(--text);
    background: var(--button-hover);
  }

  .update-notes {
    margin-top: 3px;
    color: var(--secondary);
    font-size: 9px;
    line-height: 12px;
  }

  .update-notes summary {
    width: fit-content;
    color: var(--secondary);
    cursor: pointer;
    font-size: 10px;
  }

  .update-notes p {
    max-height: 72px;
    margin: 5px 0 0;
    padding-right: 3px;
    overflow-y: auto;
    white-space: pre-line;
  }

  .update-error {
    gap: 1px;
    margin-top: 4px;
    padding: 6px 7px;
    border-radius: 7px;
    color: var(--error);
    background: var(--error-bg);
  }

  .update-error strong {
    font-size: 10px;
  }

  .update-error small {
    color: var(--error);
    font-size: 9px;
    line-height: 12px;
  }

  .update-progress {
    height: 4px;
    margin-top: 4px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--meter-track);
  }

  .update-progress span {
    display: block;
    min-width: 8%;
    height: 100%;
    border-radius: inherit;
    background: var(--meter-fill);
    transition: width 160ms ease;
  }
</style>
