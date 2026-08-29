<script lang="ts">
  import { product } from "./product";
</script>

<svelte:head>
  <title>{product.name}</title>
</svelte:head>

<main class="shell">
  <section class="status-panel" aria-labelledby="workspace-title">
    <p class="eyebrow">Local release candidate workspace</p>
    <h1 id="workspace-title">{product.name}</h1>
    <p class="version">Version {product.version}</p>
    <p class="boundary">
      Runtime controls and synthesis state live outside this WebView.
    </p>
  </section>
</main>
