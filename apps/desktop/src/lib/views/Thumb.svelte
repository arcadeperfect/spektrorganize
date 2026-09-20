<script lang="ts" module>
  // One queue shared by every tile, so a slow card is not hammered by hundreds of requests.
  const MAX = 4;
  let running = 0;
  const queue: (() => void)[] = [];
  function schedule(fn: () => Promise<void>) {
    const run = () => {
      running++;
      fn().finally(() => {
        running--;
        queue.shift()?.();
      });
    };
    if (running < MAX) run();
    else queue.push(run);
  }
</script>

<script lang="ts">
  import { api } from "../api";

  let { group }: { group: number } = $props();
  let url = $state<string | null>(null);
  let failed = $state(false);

  $effect(() => {
    const id = group;
    let cancelled = false;
    schedule(async () => {
      if (cancelled) return;
      try {
        const p = await api.thumbnail(id);
        if (!cancelled) url = p ? api.fileUrl(p) : null;
        if (!p) failed = true;
      } catch {
        if (!cancelled) failed = true;
      }
    });
    return () => {
      cancelled = true;
    };
  });
</script>

{#if url}
  <img src={url} alt="" draggable="false" />
{:else if failed}
  <div class="ph muted">no preview</div>
{:else}
  <div class="ph"></div>
{/if}

<style>
  img,
  .ph {
    width: 100%;
    aspect-ratio: 3 / 2;
    object-fit: cover;
    display: block;
    background: var(--bg-3);
    border-radius: 4px;
  }
  .ph {
    display: grid;
    place-items: center;
    font-size: 11px;
  }
</style>
