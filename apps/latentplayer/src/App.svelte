<script lang="ts">
  import { product } from "./product";
</script>

<svelte:head>
  <title>{product.name}</title>
</svelte:head>

<main class="player-shell">
  <section aria-labelledby="player-title">
    <p class="eyebrow">Cartridge playback</p>
    <h1 id="player-title">{product.name}</h1>
    <p class="version">Version {product.version}</p>
    <p>Open · Play · Pause · Loop · Restart · Fullscreen</p>
  </section>
</main>
