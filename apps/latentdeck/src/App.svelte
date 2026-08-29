<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import {
    EMPTY_LIBRARY_VIEW,
    canReorderActiveMembers,
    describeCommandError,
    formatDuration,
    moveItem,
    parseTags,
    realCollections,
    shortHash,
    type CartridgeView,
    type CollectionView,
    type ImportSummary,
    type LibraryView,
    type ReindexSummary,
  } from "./library-model";
  import { product } from "./product";

  let view: LibraryView = EMPTY_LIBRARY_VIEW;
  let search = "";
  let busy = false;
  let errorMessage = "";
  let notice = "";
  let recursiveFolderImport = false;
  let newCollectionName = "";
  let renamingId: string | null = null;
  let renameDraft = "";
  let tagDrafts: Record<string, string> = {};
  let membershipTargets: Record<string, string> = {};

  let activeCollection: CollectionView | undefined;
  let persistedCollections: CollectionView[] = [];
  let memberReorderEnabled = false;
  $: activeCollection = view.collections.find(
    (collection) => collection.id === view.deckSession.activeCollectionId,
  );
  $: persistedCollections = realCollections(view.collections);
  $: memberReorderEnabled = canReorderActiveMembers(view, search);

  onMount(() => {
    void initialLoad();
  });

  async function initialLoad(): Promise<void> {
    busy = true;
    try {
      await refresh();
    } catch (error) {
      errorMessage = describeCommandError(error);
    } finally {
      busy = false;
    }
  }

  async function refresh(): Promise<void> {
    view = await invoke<LibraryView>("library_snapshot", {
      search: search.trim() === "" ? null : search,
    });
    const nextDrafts = { ...tagDrafts };
    for (const cartridge of [...view.cartridges, ...view.recent]) {
      nextDrafts[cartridge.archiveSha256] ??= cartridge.tags.join(", ");
    }
    tagDrafts = nextDrafts;
  }

  async function mutate(
    operation: () => Promise<string | void>,
    fallbackNotice: string,
  ): Promise<void> {
    if (busy) return;
    busy = true;
    errorMessage = "";
    notice = "";
    try {
      const operationNotice = await operation();
      await refresh();
      notice = operationNotice ?? fallbackNotice;
    } catch (error) {
      errorMessage = describeCommandError(error);
    } finally {
      busy = false;
    }
  }

  async function searchLibrary(): Promise<void> {
    await mutate(
      async () => undefined,
      search.trim() === "" ? "Showing full bank." : "Search applied.",
    );
  }

  async function importFiles(): Promise<void> {
    const selection = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "Latent Cartridge", extensions: ["lc"] }],
    });
    if (selection === null) return;
    const paths = Array.isArray(selection) ? selection : [selection];
    await mutate(async () => {
      const summary = await invoke<ImportSummary>("library_import_files", {
        paths,
      });
      return importNotice(summary);
    }, "Import complete.");
  }

  async function importFolder(): Promise<void> {
    const selection = await open({ multiple: false, directory: true });
    if (selection === null || Array.isArray(selection)) return;
    await mutate(async () => {
      const summary = await invoke<ImportSummary>("library_import_folder", {
        path: selection,
        recursive: recursiveFolderImport,
      });
      return importNotice(summary);
    }, "Folder import complete.");
  }

  function importNotice(summary: ImportSummary): string {
    const parts = [`${summary.accepted} accepted`];
    if (summary.rejected.length > 0)
      parts.push(`${summary.rejected.length} rejected`);
    if (summary.ignoredNonCartridges > 0) {
      parts.push(`${summary.ignoredNonCartridges} non-cartridge files ignored`);
    }
    return parts.join(" · ");
  }

  async function reindex(): Promise<void> {
    await mutate(async () => {
      const summary = await invoke<ReindexSummary>("library_reindex");
      return [
        `${summary.unchanged} unchanged`,
        `${summary.present} restored`,
        `${summary.missing} missing`,
        `${summary.invalid} invalid`,
        `${summary.contentChanged} changed`,
      ].join(" · ");
    }, "Registered paths checked.");
  }

  async function selectCollection(collectionId: string): Promise<void> {
    await mutate(async () => {
      await invoke("library_set_active_collection", { collectionId });
    }, "Active Bank changed. Loaded slots were left untouched.");
  }

  async function createCollection(): Promise<void> {
    const name = newCollectionName.trim();
    if (name === "") return;
    await mutate(async () => {
      await invoke("library_create_collection", { name });
      newCollectionName = "";
    }, "Collection created and selected.");
  }

  function beginRename(collection: CollectionView): void {
    renamingId = collection.id;
    renameDraft = collection.name;
  }

  async function commitRename(collectionId: string): Promise<void> {
    const name = renameDraft.trim();
    if (name === "") return;
    await mutate(async () => {
      await invoke("library_rename_collection", { collectionId, name });
      renamingId = null;
      renameDraft = "";
    }, "Collection renamed.");
  }

  async function deleteCollection(collection: CollectionView): Promise<void> {
    if (
      !window.confirm(
        `Delete collection “${collection.name}”? Cartridge files are not deleted.`,
      )
    ) {
      return;
    }
    await mutate(async () => {
      await invoke("library_delete_collection", {
        collectionId: collection.id,
      });
    }, "Collection membership deleted. Cartridge files were untouched.");
  }

  async function moveCollection(
    collectionId: string,
    direction: -1 | 1,
  ): Promise<void> {
    const reordered = moveItem(
      persistedCollections,
      collectionId,
      direction,
      (collection) => collection.id,
    );
    if (
      reordered.every(
        (collection, index) =>
          collection.id === persistedCollections[index]?.id,
      )
    ) {
      return;
    }
    await mutate(async () => {
      await invoke("library_reorder_collections", {
        collectionIds: reordered.map((collection) => collection.id),
      });
    }, "Collection order saved.");
  }

  function dragCartridge(event: DragEvent, archiveSha256: string): void {
    event.dataTransfer?.setData(
      "application/x-latentdeck-cartridge",
      archiveSha256,
    );
    if (event.dataTransfer !== null) event.dataTransfer.effectAllowed = "copy";
  }

  async function dropOnCollection(
    event: DragEvent,
    collection: CollectionView,
  ): Promise<void> {
    event.preventDefault();
    if (collection.isVirtual) return;
    const archiveSha256 = event.dataTransfer?.getData(
      "application/x-latentdeck-cartridge",
    );
    if (archiveSha256 === undefined || archiveSha256 === "") return;
    await addMembership(collection.id, archiveSha256);
  }

  function updateMembershipTarget(event: Event, archiveSha256: string): void {
    const select = event.currentTarget as HTMLSelectElement;
    membershipTargets = { ...membershipTargets, [archiveSha256]: select.value };
  }

  async function addSelectedMembership(
    cartridge: CartridgeView,
  ): Promise<void> {
    const collectionId = membershipTargets[cartridge.archiveSha256];
    if (collectionId === undefined || collectionId === "") return;
    await addMembership(collectionId, cartridge.archiveSha256);
  }

  async function addMembership(
    collectionId: string,
    archiveSha256: string,
  ): Promise<void> {
    await mutate(async () => {
      await invoke("library_add_membership", { collectionId, archiveSha256 });
    }, "Cartridge added. Other collection memberships were preserved.");
  }

  async function removeFromActive(cartridge: CartridgeView): Promise<void> {
    if (activeCollection === undefined || activeCollection.isVirtual) return;
    await mutate(async () => {
      await invoke("library_remove_membership", {
        collectionId: activeCollection?.id,
        archiveSha256: cartridge.archiveSha256,
      });
    }, "Cartridge removed from this collection only.");
  }

  async function moveMember(
    archiveSha256: string,
    direction: -1 | 1,
  ): Promise<void> {
    if (!memberReorderEnabled || activeCollection === undefined) return;
    const reordered = moveItem(
      view.cartridges,
      archiveSha256,
      direction,
      (cartridge) => cartridge.archiveSha256,
    );
    if (
      reordered.every(
        (cartridge, index) =>
          cartridge.archiveSha256 === view.cartridges[index]?.archiveSha256,
      )
    ) {
      return;
    }
    await mutate(async () => {
      await invoke("library_reorder_members", {
        collectionId: activeCollection?.id,
        archiveSha256Order: reordered.map(
          (cartridge) => cartridge.archiveSha256,
        ),
      });
    }, "Cartridge order saved.");
  }

  async function toggleFavorite(cartridge: CartridgeView): Promise<void> {
    await mutate(
      async () => {
        await invoke("library_set_favorite", {
          archiveSha256: cartridge.archiveSha256,
          favorite: !cartridge.favorite,
        });
      },
      cartridge.favorite ? "Removed from favorites." : "Added to favorites.",
    );
  }

  function updateTagDraft(event: Event, archiveSha256: string): void {
    const input = event.currentTarget as HTMLInputElement;
    tagDrafts = { ...tagDrafts, [archiveSha256]: input.value };
  }

  async function saveTags(cartridge: CartridgeView): Promise<void> {
    const tags = parseTags(tagDrafts[cartridge.archiveSha256] ?? "");
    await mutate(async () => {
      await invoke("library_set_tags", {
        archiveSha256: cartridge.archiveSha256,
        tags,
      });
      tagDrafts = { ...tagDrafts, [cartridge.archiveSha256]: tags.join(", ") };
    }, "Tags saved.");
  }

  async function markRecent(cartridge: CartridgeView): Promise<void> {
    await mutate(async () => {
      await invoke("library_mark_recent", {
        archiveSha256: cartridge.archiveSha256,
      });
    }, "Cartridge moved to Recent.");
  }
</script>

<svelte:head>
  <title>{product.name} — Library</title>
</svelte:head>

<main class="instrument-shell" aria-busy={busy}>
  <header class="top-rail">
    <div class="identity">
      <span class="status-lamp" class:working={busy}></span>
      <div>
        <p class="eyebrow">Latent media instrument · local library</p>
        <h1>{product.name}</h1>
      </div>
      <span class="version">v{product.version}</span>
    </div>
    <div class="counter-block" aria-label="Indexed cartridge count">
      <span>INDEX</span>
      <strong>{view.totalIndexed.toString().padStart(3, "0")}</strong>
      <small>cartridges</small>
    </div>
  </header>

  <section class="command-rail" aria-label="Library commands">
    <button
      class="primary"
      type="button"
      onclick={() => void importFiles()}
      disabled={busy}
    >
      Import .LC files
    </button>
    <button type="button" onclick={() => void importFolder()} disabled={busy}
      >Import folder</button
    >
    <label class="toggle-control" title="Off means direct children only">
      <input
        type="checkbox"
        bind:checked={recursiveFolderImport}
        disabled={busy}
      />
      <span>Recursive folder</span>
    </label>
    <button type="button" onclick={() => void reindex()} disabled={busy}
      >Reindex registered</button
    >
    <form
      class="search-unit"
      onsubmit={(event) => {
        event.preventDefault();
        void searchLibrary();
      }}
    >
      <label for="library-search">Search</label>
      <input
        id="library-search"
        bind:value={search}
        maxlength="256"
        placeholder="ID, tag, codec, filename"
      />
      <button type="submit" disabled={busy}>Apply</button>
    </form>
  </section>

  {#if errorMessage !== ""}
    <div class="message error" role="alert">{errorMessage}</div>
  {:else if notice !== ""}
    <div class="message notice" role="status">{notice}</div>
  {/if}

  <div class="work-surface">
    <aside class="bank-panel" aria-labelledby="banks-title">
      <div class="panel-heading">
        <div>
          <p class="eyebrow">Deck source selector</p>
          <h2 id="banks-title">Collections <span>/ Banks</span></h2>
        </div>
        <span class="bank-readout">{activeCollection?.name ?? "ALL"}</span>
      </div>

      <div class="bank-list">
        {#each view.collections as collection, index (collection.id)}
          <div
            class="bank-row"
            class:active={collection.id === view.deckSession.activeCollectionId}
            class:virtual={collection.isVirtual}
            role="group"
            aria-label={`${collection.name} collection`}
            ondragover={(event) => {
              if (!collection.isVirtual) event.preventDefault();
            }}
            ondrop={(event) => void dropOnCollection(event, collection)}
          >
            <button
              class="bank-select"
              type="button"
              onclick={() => void selectCollection(collection.id)}
              aria-pressed={collection.id ===
                view.deckSession.activeCollectionId}
              disabled={busy}
            >
              <span class="bank-index"
                >{collection.isVirtual
                  ? "V"
                  : String(index - 1).padStart(2, "0")}</span
              >
              <span class="bank-name">{collection.name}</span>
              <span class="bank-count">{collection.memberCount}</span>
            </button>
            {#if !collection.isVirtual}
              <div
                class="micro-controls"
                aria-label={`Controls for ${collection.name}`}
              >
                <button
                  type="button"
                  onclick={() => void moveCollection(collection.id, -1)}
                  disabled={busy || collection.position === 0}
                  aria-label="Move collection up">↑</button
                >
                <button
                  type="button"
                  onclick={() => void moveCollection(collection.id, 1)}
                  disabled={busy ||
                    collection.position === persistedCollections.length - 1}
                  aria-label="Move collection down">↓</button
                >
                <button
                  type="button"
                  onclick={() => beginRename(collection)}
                  disabled={busy}
                  aria-label="Rename collection">R</button
                >
                <button
                  class="danger-control"
                  type="button"
                  onclick={() => void deleteCollection(collection)}
                  disabled={busy}
                  aria-label="Delete collection">×</button
                >
              </div>
            {/if}
            {#if renamingId === collection.id}
              <form
                class="inline-editor"
                onsubmit={(event) => {
                  event.preventDefault();
                  void commitRename(collection.id);
                }}
              >
                <input
                  bind:value={renameDraft}
                  maxlength="128"
                  aria-label="New collection name"
                />
                <button type="submit" disabled={busy}>Save</button>
                <button
                  type="button"
                  onclick={() => {
                    renamingId = null;
                  }}
                  disabled={busy}>Cancel</button
                >
              </form>
            {/if}
          </div>
        {/each}
      </div>

      <form
        class="create-bank"
        onsubmit={(event) => {
          event.preventDefault();
          void createCollection();
        }}
      >
        <label for="new-collection">New collection</label>
        <div>
          <input
            id="new-collection"
            bind:value={newCollectionName}
            maxlength="128"
            placeholder="Collection name"
          />
          <button
            type="submit"
            disabled={busy || newCollectionName.trim() === ""}>Create</button
          >
        </div>
      </form>

      <p class="drop-note">
        Drag any cartridge onto a real collection. Existing memberships remain.
      </p>
      <div class="session-contract">
        <span>Active Bank</span>
        <strong>{activeCollection?.name ?? "All Cartridges"}</strong>
        <small
          >{view.deckSession.loadedSlots.length} loaded slots · bank changes never
          unload them</small
        >
      </div>
    </aside>

    <section class="browser-panel" aria-labelledby="browser-title">
      <div class="panel-heading browser-heading">
        <div>
          <p class="eyebrow">Cartridge browser</p>
          <h2 id="browser-title">
            {activeCollection?.name ?? "All Cartridges"}
          </h2>
        </div>
        <div class="scope-readout">
          <strong>{view.cartridges.length}</strong>
          <span>shown / {view.activeMemberCount} bank</span>
        </div>
      </div>

      {#if search.trim() !== ""}
        <div class="filter-strip">
          Filter: <strong>{search}</strong>
          <button
            type="button"
            onclick={() => {
              search = "";
              void searchLibrary();
            }}>Clear</button
          >
        </div>
      {/if}

      {#if view.cartridges.length === 0}
        <div class="empty-bay">
          <div class="empty-reel">LC</div>
          <h3>No cartridges in this view</h3>
          <p>
            Import explicit files or a selected folder. LatentDeck never scans
            your drives.
          </p>
        </div>
      {:else}
        <div class="cartridge-grid">
          {#each view.cartridges as cartridge, index (cartridge.archiveSha256)}
            <article
              class="cartridge-card"
              class:unavailable={cartridge.availability !== "present"}
              draggable="true"
              ondragstart={(event) =>
                dragCartridge(event, cartridge.archiveSha256)}
            >
              <div class="cartridge-spine">
                <span>LC</span>
                <small>{String(index + 1).padStart(2, "0")}</small>
              </div>
              <div class="cartridge-body">
                <header>
                  <div>
                    <p class="file-name">
                      {cartridge.paths[0]?.fileName ?? "Unavailable cartridge"}
                    </p>
                    <p class="hash" title={cartridge.archiveSha256}>
                      {shortHash(cartridge.archiveSha256)}
                    </p>
                  </div>
                  <button
                    class="favorite"
                    class:selected={cartridge.favorite}
                    type="button"
                    aria-label={cartridge.favorite
                      ? "Remove favorite"
                      : "Add favorite"}
                    aria-pressed={cartridge.favorite}
                    onclick={() => void toggleFavorite(cartridge)}
                    disabled={busy}>★</button
                  >
                </header>

                <div class="format-line">
                  <span>{cartridge.codecFamily}</span>
                  <span>{cartridge.decodedWidth}×{cartridge.decodedHeight}</span
                  >
                  <span>{cartridge.decodedFrameCount}f</span>
                  <span
                    >{formatDuration(
                      cartridge.durationNumerator,
                      cartridge.durationDenominator,
                    )}</span
                  >
                </div>

                <div
                  class="path-state"
                  class:warning={cartridge.availability !== "present"}
                >
                  <span class="state-dot"></span>
                  <span
                    >{cartridge.paths[0]?.state ?? cartridge.availability}</span
                  >
                  {#if cartridge.paths[0]?.warningCode}
                    <small>{cartridge.paths[0].warningCode}</small>
                  {/if}
                </div>

                <div class="tag-list" aria-label="Tags">
                  {#each cartridge.tags as tag}<span>{tag}</span>{/each}
                  {#if cartridge.tags.length === 0}<em>untagged</em>{/if}
                </div>

                <form
                  class="tag-editor"
                  onsubmit={(event) => {
                    event.preventDefault();
                    void saveTags(cartridge);
                  }}
                >
                  <input
                    value={tagDrafts[cartridge.archiveSha256] ??
                      cartridge.tags.join(", ")}
                    oninput={(event) =>
                      updateTagDraft(event, cartridge.archiveSha256)}
                    maxlength="512"
                    placeholder="tags, comma separated"
                    aria-label={`Tags for ${cartridge.paths[0]?.fileName ?? cartridge.cartridgeId}`}
                  />
                  <button type="submit" disabled={busy}>Tags</button>
                </form>

                <div class="membership-editor">
                  <select
                    aria-label="Collection to add cartridge to"
                    value={membershipTargets[cartridge.archiveSha256] ?? ""}
                    onchange={(event) =>
                      updateMembershipTarget(event, cartridge.archiveSha256)}
                    disabled={busy || persistedCollections.length === 0}
                  >
                    <option value="">Add to collection…</option>
                    {#each persistedCollections as collection}<option
                        value={collection.id}>{collection.name}</option
                      >{/each}
                  </select>
                  <button
                    type="button"
                    onclick={() => void addSelectedMembership(cartridge)}
                    disabled={busy ||
                      !membershipTargets[cartridge.archiveSha256]}>Add</button
                  >
                </div>

                <footer class="card-controls">
                  <button
                    type="button"
                    onclick={() => void markRecent(cartridge)}
                    disabled={busy}>Use / Recent</button
                  >
                  {#if activeCollection !== undefined && !activeCollection.isVirtual}
                    <button
                      type="button"
                      onclick={() => void removeFromActive(cartridge)}
                      disabled={busy}>Remove here</button
                    >
                    <span class="order-pair">
                      <button
                        type="button"
                        onclick={() =>
                          void moveMember(cartridge.archiveSha256, -1)}
                        disabled={busy || !memberReorderEnabled || index === 0}
                        aria-label="Move cartridge earlier">←</button
                      >
                      <button
                        type="button"
                        onclick={() =>
                          void moveMember(cartridge.archiveSha256, 1)}
                        disabled={busy ||
                          !memberReorderEnabled ||
                          index === view.cartridges.length - 1}
                        aria-label="Move cartridge later">→</button
                      >
                    </span>
                  {/if}
                </footer>
              </div>
            </article>
          {/each}
        </div>
      {/if}
    </section>

    <aside class="recent-panel" aria-labelledby="recent-title">
      <div class="panel-heading compact">
        <div>
          <p class="eyebrow">Session memory</p>
          <h2 id="recent-title">Recent</h2>
        </div>
      </div>
      {#if view.recent.length === 0}
        <p class="recent-empty">Use a cartridge to place it here.</p>
      {:else}
        <ol class="recent-list">
          {#each view.recent as cartridge, index (cartridge.archiveSha256)}
            <li>
              <span>{String(index + 1).padStart(2, "0")}</span>
              <button
                type="button"
                onclick={() => void markRecent(cartridge)}
                title={cartridge.paths[0]?.path ?? cartridge.cartridgeId}
              >
                <strong>{cartridge.paths[0]?.fileName ?? "Unavailable"}</strong>
                <small>{shortHash(cartridge.archiveSha256)}</small>
              </button>
              {#if cartridge.favorite}<b aria-label="Favorite">★</b>{/if}
            </li>
          {/each}
        </ol>
      {/if}
      <div class="safety-plate">
        <strong>LOCAL INDEX</strong>
        <p>
          .lc files stay where you put them. Deleting a collection never deletes
          media.
        </p>
      </div>
    </aside>
  </div>
</main>
