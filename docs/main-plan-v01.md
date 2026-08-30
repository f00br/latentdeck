# LatentDeck v0.1 — полный локальный release candidate

## Итог и критерий готовности

Цель — собрать на `main` полноценный локальный `0.1.0` без remote, push и Git-тега. Версия считается достигнутой только после полного цикла:

`Create → Cartridge → Play → D2/Q4 Synthesis → Snapshot/Live Resample → Play Again → Spout`.

Зафиксированные решения:

- `.lc` и realtime signal contract — стабильный центр; UI, Deck, codec worker и operators заменяемы.
- В v0.1 входят LatentPlayer, LD-D2, LD-Q4, Comfy recorder, packer/SDK, collections, snapshot и live resample, native output и Spout2.
- Коллекции many-to-many: cartridge может входить в несколько коллекций.
- Одна Deck-сессия имеет одну активную коллекцию; доступны виртуальные `All Cartridges` и `Unassigned`.
- Концепты остаются локальными, не попадают в Git и используются только как визуальное настроение после фиксации поведения.
- Лицензия оригинального кода и документации — Apache-2.0.
- Audio metadata/payload сохраняются, но playback и synthesis audio отсутствуют.
- InteractionNet, realtime Projector/HQ VAE, prompt control, timeline, scenes, MIDI/OSC, NDI, mapping и scratch исключены.
- Arbitrary seek не показываем: доступны Play, Pause, Loop и Restart; loop/reset явно сбрасывает causal decoder state.

Текущая отправная точка: `main` без первого коммита, весь scaffold untracked, public-tree audit проходит, H3 Lab работает на `127.0.0.1:8190`, а три реальные H3 AV latent A/B/C уже есть локально. RunPod для первых этапов не нужен.

## Архитектура и публичные контракты

```text
Comfy Recorder / LC Packer
            ↓
       .lc Cartridge
            ↓
 Rust Cartridge SDK ──→ SQLite Library / Collections
            ↓
     Rust Core / Control
            ↓ versioned MessagePack over Named Pipe
 isolated H3 PyTorch worker
            ↓
 D2/Q4 operators → post-operator RESAMPLE .lc
            ↓
      TAEHV / taeh3 decode
            ↓ shared-memory bounded ring
       native wgpu renderer
            ↓
       Window / Fullscreen / Spout2
```

### Toolchain

- Закрепить Node `24.x LTS`; установленный Node 25 уже EOL и не должен попасть в reproducible build. [Node.js release status](https://nodejs.org/en/about/previous-releases).
- Зафиксировать pnpm 11, Tauri 2.11, Svelte 5, Vite 8, TypeScript 7 и Tailwind 4.3 через manifests и lock-файл. Tauri использует MSVC Build Tools и WebView2; необходимые Visual Studio Build Tools на машине присутствуют. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), [Svelte](https://svelte.dev/docs/svelte/overview), [Tailwind](https://tailwindcss.com/docs/installation/tailwind-cli).
- Rust 1.93/Cargo workspace; `wgpu` 30.x, Tokio, serde, tracing и SQLite. [wgpu releases](https://github.com/gfx-rs/wgpu/releases).
- H3 worker: Python 3.13, uv lock, PyTorch `2.13.0+cu130`, Safetensors. Эта линия уже работает в локальной лаборатории и имеет официальный Windows cu130 build. [PyTorch 2.13](https://pytorch.org/blog/pytorch-2-13-release-blog/), [Windows cu130 wheels](https://download.pytorch.org/whl/cu130/torch/).
- H3 Codec Pack устанавливается отдельно от приложений. Он содержит изолированный runtime и adapter, но не H3 Transformer, native HQ VAE или `taeh3` weight. Пользователь выбирает weight явно; Codec Manager показывает source, license, hash и compatibility.

### `.lc` 0.1

Физический контейнер — deterministic ZIP64 без сжатия:

```text
manifest.json
payloads/h3.safetensors
preview.webp             # optional
```

- H3 Safetensors содержит `video` и optional `audio`; никаких MP4, Python modules или scripts.
- Manifest фиксирует spec/profile versions, cartridge ID, codec, tensors, per-stream storage/runtime dtype, shapes, timing, decoded frame count/resolution, payload hashes, provenance, parents и operation history.
- H3 profile принимает visual `[1,24,T,H,W]` и audio `[1,32,2,T_audio]`; F32 storage импортируется без потери, runtime visual переводится в FP16. Resampled visual по умолчанию сохраняется в фактическом post-operator FP16, audio сохраняет исходный dtype.
- Codec adapter владеет H3 cadence, включая проверенные `T=32 → 107 frames`, `T=72 → 243 frames` и streaming contract `5 slots → 17 usable frames`.
- Reader никогда не распаковывает archive на диск. Он отклоняет duplicate/path-traversal/encrypted/compressed entries, oversized headers/payloads, overflow размеров, неизвестные версии, hash mismatch, forbidden dtype, неверные shapes и NaN/Inf до GPU allocation. Safetensors исключает pickle-style execution, но сам по себе не проверяет NaN/Inf, поэтому эти проверки остаются обязанностью SDK. [Safetensors format](https://github.com/huggingface/safetensors).
- Никакого скрытого crop, resize, re-encode или dtype conversion вне явно описанного profile/runtime cast.

### Общие API

| Контракт | Решение v0.1 |
|---|---|
| Cartridge SDK | Один Rust implementation: read, write, validate, inspect, hash; Python binding через PyO3/maturin; CLI `latentdeck-cartridge pack/validate/inspect/hash`. |
| Library | SQLite индексирует пути, hashes, metadata, favorites, tags, recent и collections; сами `.lc` остаются обычными файлами. |
| Collections | Flat many-to-many membership с ручным порядком; `All` и `Unassigned` виртуальны; удаление collection никогда не удаляет cartridge-файлы и не выгружает уже играющий slot. |
| Control API | Typed commands/events с command ID, schema version, ack/error; UI не владеет clock или latent math. |
| Worker IPC | Length-prefixed MessagePack через Windows Named Pipe; tensors не ходят через control IPC; RGB frames передаются через bounded shared-memory ring. |
| Codec API | Handshake, inspect/load/unload, timing, slot transport, process/decode/reset, resample и metrics. |
| Operator API | Explicitly installed trusted Python operators; descriptor + supported profiles/controls + deterministic `process_slot`. Cartridge никогда не устанавливает operator-код. |
| Deck preset | Versioned JSON: deck type, active collection, slot cartridge IDs/hashes, controls, routing и deterministic seed. Missing cartridges дают warning, а не скрытую замену. |

### Общая geometry/scale policy

- Геометрию определяет общий Core-контракт `SignalGeometry`, а не конкретная
  дека или её faceplate. Library, Player, встроенные и сторонние Decks получают
  одни и те же validated codec/profile, latent `T/H/W`, decoded extent и timing.
- Library и Collections могут одновременно содержать portrait, landscape,
  square и любые другие валидные размеры. Player показывает intrinsic decoded
  extent через centered aspect-fit с чёрными полями, без stretch или crop.
- LD-D2 и LD-Q4 используют Core policy `spatial_synthesis`: для прямого
  смешивания обязаны совпадать codec/profile/version, runtime dtype/layout,
  latent `H/W`, decoded `W/H`, timing contract/version и frame rate. Независимые
  playheads позволяют разный latent `T` и разную длину клипа.
- Одинаковое бытовое название aspect ratio (`16:9`, `9:16`) само по себе ничего
  не разрешает: решение принимается по точной validated геометрии. Portrait и
  landscape с разными spatial grids напрямую не смешиваются.
- Никакой Deck не делает скрытый resize/crop/re-encode. Explicit Toolkit
  Align/Crop создаёт новый provenance-bearing `.lc`; только после этого он может
  пройти общий compatibility gate. Сторонняя Deck обязана выбрать и использовать
  опубликованную Core policy вместо собственной неявной логики. Полный контракт
  фиксируется в `spec/deck-api/README.md`.

### D2, Q4 и resampling

- LD-D2: независимые A/B playheads, Linear baseline, XS1–XS4 и обязательный XS5 TOPK/Sinkhorn с `HYBRIDIZE`/`INTERACT`. `ROUTING` выбирает structural carrier A или B.
- LD-Q4: один явно выбранный Carrier и три Donors. Каждый donor маршрутизируется относительно неизменённого carrier; routed states накапливаются в фиксированном B/C/D порядке.
- Donor influences нормализуются предсказуемо: общий `INTERACTION` задаёт суммарную силу, независимые weights — относительное распределение. Треугольное influence field является macro над тремя weights, а не отдельным алгоритмом.
- Q4 использует batched/reused carrier affinity, bounded top-k/iterations и полный latent grid; понижать разрешение или молча менять алгоритм ради FPS запрещено.
- `CHAOS` — только seeded deterministic perturbation; `0` означает exact unchanged path, seed сохраняется в preset/resample provenance.

Resample имеет два режима:

- `Snapshot`: один полный carrier-cycle с текущими фиксированными controls.
- `Live Capture`: запись меняющегося post-operator latent stream; старт на следующей codec-valid boundary, остановка пользователем, данные пишутся в bounded temporary spool, а не копятся в RAM.

Оба режима пишут `.partial`, атомарно переименовывают только после успешной валидации и сразу импортируют результат в Library. Snapshot копирует audio structural carrier при полном совпадении timing. Live capture копирует audio только при точном совпадении длительности/temporal mapping; иначе audio отсутствует с явным `audio_policy`, без скрытого crop/stretch.

## Этапы и локальные коммиты

1. **Public-safe baseline**

   - Добавить Apache-2.0 `LICENSE`.
   - Игнорировать четыре concept PNG, оставить их физически на месте и обновить документы так, чтобы public clone их не ожидал.
   - Исправить устаревшую таблицу hash/size для `ui_concept_main.png` удалением локального asset inventory из публичной документации.
   - Первый commit: `chore(repo): establish public-safe bootstrap`.

2. **Reproducible workspace**

   - Создать Cargo/pnpm/uv workspaces, pin toolchains, lock dependencies и собрать минимальные Tauri/Rust/Python smoke targets.
   - Добавить форматирование, lint/test commands и локальный aggregate check.
   - Commit: `chore(workspace): initialize reproducible toolchains`.

3. **Cartridge standard и SDK**

   - Зафиксировать LC Spec 0.1, H3 Profile 0.1, archive layout, validation limits, compatibility errors и versioning.
   - Реализовать Rust SDK/CLI, deterministic writer и synthetic temporary fixtures.
   - Проверить pack/inspect/validate/hash roundtrip и malformed archive matrix.
   - Commit: `feat(cartridge): implement LC 0.1 standard and SDK`.

4. **Create path**

   - Добавить Python bindings, raw Safetensors packer и независимый `Save Latent Cartridge` Comfy node с passthrough latent output.
   - Конвертировать внешние B/C/A в `.lc` вне репозитория; сверить tensor schemas, hashes и metadata Rust/Python implementations.
   - Commit: `feat(authoring): add H3 packer and Comfy recorder`.

5. **Worker, Player и native presentation**

   - Реализовать codec-pack discovery, versioned worker protocol, process supervision, shared-memory ring и `wgpu` DX12 renderer.
   - LatentPlayer: Open, Play, Pause, Loop, Restart, native window/fullscreen, progress и codec/error status; audio controls отсутствуют.
   - Worker crash переводит Player в Paused/Error, очищает causal state и допускает явный restart без падения приложения.
   - Commit: `feat(player): play H3 cartridges through isolated worker`.

6. **Library и Collections**

   - Добавить explicit folder/file import, incremental indexing, search и SQLite migrations; никакого скана диска.
   - Реализовать create/rename/delete/reorder collection, drag-to-collection, multiple membership, manual cartridge order, `All` и `Unassigned`.
   - В Deck UI API термин остаётся `Collection`; аппаратный selector отображается как `Bank`.
   - Commit: `feat(library): add shared cartridge collections`.

7. **LD-D2 и полный первый цикл**

   - Вынести scheduler/control state из UI, перенести проверенную operator math чисто и с provenance review.
   - Реализовать Linear, XS1–XS5, realtime controls, compatibility gate, collection-scoped slot picker, Snapshot и Live Capture.
   - Acceptance slice: B+C → Linear/XS5 → new `.lc` → validator → Library → LatentPlayer.
   - Commit: `feat(deck): complete LD-D2 synthesis and resampling`.

8. **Расширение private corpus и LD-Q4**

   - До Q4 использовать локальные данные. Затем узко запустить текущий CPU-only RunPod только для SSH/download: live re-discovery, Continuum manifests/chunks inventory, download, SHA-256/Safetensors validation, stop если pod был запущен этой операцией.
   - Не использовать устаревший GPU-oriented launcher, не сканировать весь `/workspace`, не запускать generation.
   - Минимум для release acceptance — четыре независимых совместимых cartridges; целевой private corpus — 12 single-chunk cartridges для collections/Q4 testing.
   - Реализовать Carrier + 3 Donors, triangular influence macro, batched XS5 и Q4 resample/reload.
   - Commit: `feat(deck): add deterministic LD-Q4 carrier-donor synthesis`.

9. **Spout, Toolkit и UI finish**

   - Share native DX12 texture через Spout2 без encode; sender enable/name/status и receiver proof обязательны. Spout2 поддерживает DX12 и распространяется под BSD-2-Clause, что фиксируется в third-party notices. [Spout2](https://github.com/leadedge/Spout2).
   - Clean-room перенести в Comfy Toolkit XS1–XS5, FAST/HQ comparison hooks и offline Projector; не переносить InteractionNet checkpoint, private workflows, data, outputs или абсолютные пути.
   - Добавить один внешний example operator, Codec/Operator/Deck schema docs.
   - Только после функциональной стабилизации адаптировать UI к instrument-like концептам: main Library, разные D2/Q4 faceplates и простой Player. Не копировать fictional X6/L8, audio controls, размеры, labels или инженерные схемы концептов.
   - Commits: `feat(output): add Spout2` и `feat(toolkit): publish operator research surface`.

10. **Release hardening**

   - Worker recovery, atomic resampling, diagnostic bundle, structured logs, DB backup/migration, installer/uninstall/upgrade и отдельный H3 Codec Pack.
   - Добавить local-equivalent CI scripts, GitHub workflow definitions, dependency/license inventory, SBOM и third-party notices для taehv/Spout.
   - Подготовить оригинальные screenshots уже работающего приложения вместо local-only concept PNG.
   - Финальный commit: `chore(release): prepare LatentDeck 0.1.0`.
   - Remote, push, release tag и GitHub artifacts не создавать без отдельной команды владельца.

## Тесты и exit gates v0.1

### Contract и security

- Roundtrip каждого manifest/payload variation: visual-only, AV, F32 import, FP16 resample, genealogy и optional preview.
- Reject: truncated/duplicate/path-traversal archive, hash mismatch, oversized/overflowing tensor, wrong dtype/channels/batch, NaN/Inf, unsupported spec/profile и executable-like entries.
- Cartridge никогда не запускает код; community operator загружается только через отдельную explicit installation.
- Public tests генерируют synthetic tensors во временной директории. Реальные A/B/C, `.lc`, weights и remote manifests остаются private opt-in E2E data.

### Functional E2E

- Raw H3 → Packer и Comfy Recorder → одинаково валидные `.lc`.
- Player воспроизводит 32→107 и 72→243 cases, корректно Pause/Loop/Restart и безопасно сообщает о missing codec.
- Cartridge входит в несколько collections; порядок стабилен; смена active bank не выгружает играющие slots; `All` всегда показывает полный индекс.
- D2 показывает очевидно различимые Linear и XS5 paths, после чего Snapshot и Live Capture сразу проигрываются снова.
- Q4 принимает четыре compatible cartridges, позволяет менять carrier и независимые donor influences, а повтор одинаковых inputs/seed/events даёт одинаковый latent output.
- Spout receiver получает правильное имя, размер, формат, последовательность кадров и прекращение sender без зависания.

### Realtime/performance

На RTX 4070 12 GB при 448×800/24 fps:

- D2 Linear, D2 XS5, Q4 TOPK и Q4 Sinkhorn проходят отдельный 30-minute soak.
- Средний output держится в диапазоне 23.9–24.1 fps; интервалы свыше двух кадров составляют менее 0.5% после warm-up.
- Control-to-effect p95 ≤200 ms.
- После warm-up нет растущей очереди, progressive RAM/VRAM leak или незавершённых `.partial`.
- Если Q4 не выполняет gate, оптимизируется тот же batched algorithm; hidden downscale, RGB fallback или исключение donor не допускаются.

### Packaging/release

- Clean install на отдельной Windows 11 x64 + NVIDIA системе без ComfyUI.
- App installer и Codec Pack обновляются/удаляются независимо; weights не входят ни в Git, ни в installer.
- Codec missing/incompatible/source/license states проверены UI E2E.
- Перед каждым commit: targeted tests, `git diff --cached --check`, точный staged review, `Test-PublicTree.ps1` и `git status --short`.
- Перед будущим push: полный public release checklist, archive inspection, clean history scan, SBOM/license review, Win11 result и explicit owner authorization. Подписанный публичный installer остаётся отдельным publication gate, если сертификат ещё не предоставлен.
