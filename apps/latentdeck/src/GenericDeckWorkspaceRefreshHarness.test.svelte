<script lang="ts">
  import GenericDeckWorkspace from "./GenericDeckWorkspace.svelte";
  import type { DeckUiModel } from "./deck-ui-model";
  import type { LibraryView } from "./library-model";

  export let model: DeckUiModel;
  export let initialModels: readonly DeckUiModel[];
  export let library: LibraryView;

  let models = initialModels;

  export function replaceModels(next: readonly DeckUiModel[]): void {
    models = next;
  }
</script>

<GenericDeckWorkspace
  {model}
  {models}
  {library}
  active={false}
  registerLeave={() => undefined}
/>
