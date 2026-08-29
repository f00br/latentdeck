# LatentDeck 0.1 product and architecture baseline

## Approved implementation decisions

This section records the owner's release-candidate decisions. It is
authoritative when exploratory or conditional wording later in this document
conflicts with it.

- The local target is a complete `0.1.0` on `main`, with local commits only.
  Creating a remote, pushing, tagging, or publishing artifacts requires a
  separate explicit authorization.
- Original LatentDeck code and documentation use Apache-2.0. Codec assets,
  model weights, cartridges, external operators, and third-party media retain
  independent terms.
- Local concept PNGs are ignored. They supply broad visual mood only, after
  behavior is proven; public release materials use original product
  screenshots.
- Collections are flat many-to-many sets with manual ordering. A Deck session
  has one active collection, plus virtual `All Cartridges` and `Unassigned`
  views. A playing slot survives collection changes or deletion.
- Spout2 native texture output is required in 0.1.
- Both `Snapshot` and bounded-spool `Live Capture` resampling are required.
  Results are written as `.partial`, validated, then atomically renamed and
  imported.
- Transport exposes Play, Pause, Loop, and Restart. Arbitrary seek and scratch
  are absent; loop and restart explicitly reset causal decoder state.
- Worker control uses versioned, length-prefixed MessagePack over Windows Named
  Pipes. Tensors never travel through control IPC; decoded frames use a bounded
  shared-memory ring.
- The reproducible baseline is Node 24 LTS, pnpm 11, Tauri 2.11, Svelte 5,
  Vite 8, TypeScript 7, Tailwind 4.3, Rust 1.93, `wgpu` 30.x, Python 3.13,
  and PyTorch 2.13.0 with CUDA 13.0.
- Tests generate synthetic tensors in temporary directories. Real cartridges,
  latents, weights, workflows, and remote manifests remain private opt-in E2E
  inputs and never become committed golden fixtures.
- InteractionNet, realtime Projector/native HQ VAE, prompt control, timeline,
  scenes, MIDI/OSC, NDI, mapping, and scratch are outside 0.1.

## Original product rationale

1. **Название проекта:** `LatentDeck`.
2. **Название носителя:** `Latent Cartridge`.
3. **Расширение:** `.lc`.
4. **Главная идея релиза:** не «AI VJ», а **playable latent media + realtime latent-domain synthesis**.
5. **Формат** **`.lc`** **codec-neutral.** H3 — первый поддерживаемый codec profile, но не часть определения формата:
   `codec_family = minimax_h3`.
6. **Cartridge хранит настоящий latent**, а не MP4 внутри контейнера. Для H3 — visual latent и, если имеется, audio latent.
7. **Audio сохраняем в формате уже сейчас**, но **не воспроизводим и не обрабатываем в 0.1**. Это scope следующей версии.
8. **Cartridge metadata обязательна:** версия формата, codec, tensor schema, dtype, dimensions, timing/fps, duration, hashes, provenance.
9. **Поддержать genealogy:** `parent_cartridges` + `operation_history`, потому что результат resampling может стать новым `.lc`.
10. **В первом публичном релизе представлены две конфигурации Deck:**
    **Dual Deck** — 2 активных cartridge-слота A/B;
    **Quad Deck** — 4 активных cartridge-слота A/B/C/D.
    Dual Deck сохраняем как основной точный двухисточниковый mixer. Quad Deck использует те же базовые принципы latent mixing, адаптированные к четырём источникам; для первой версии не требуется изобретать отдельную сложную методологию четырёхстороннего синтеза.
11. **Cartridge Browser:** можно иметь сколько угодно `.lc` в библиотеке; одновременно в synthesis участвуют **2 cartridge в Dual Deck или 4 cartridge в Quad Deck**.
12. **Transport минимум:** Load, Play, Pause, Loop. Seek — только настолько, насколько сейчас корректно работает causal decoder. Scratch пока не обещаем.
13. **Realtime:** параметры действуют непосредственно на проходящий latent stream без `render → MP4 → playback`. Это обязательная характеристика LatentDeck.
14. **Baseline Mixer:** обычный `Linear A/B` для Dual Deck и простой эквивалентный linear multi-source mix для Quad Deck. Он нужен как контрольная точка — пользователь должен видеть разницу между обычным dissolve/mix и latent synthesis.
15. **Главный mixer первого релиза:** нынешний content-aware `Affinity / Sinkhorn Transport`. Для Quad Deck агент должен спроектировать простое и понятное расширение этого же принципа на 4 источника, без новой ML-архитектуры.
16. **Два основных режима:** `HYBRIDIZE` и `INTERACT`.
17. **Основные performance controls:** примерно
    для Dual Deck — `A/B MIX`,
    для Quad Deck — управление вкладом `A/B/C/D`,
    плюс общие `INTERACTION`, `PRESERVE`, `ROUTING`, `TEMPERATURE`, `CHAOS`.
    Технические параметры Sinkhorn/top-k прячем в Advanced.
18. **XS1–XS4 не выбрасываем**, но рассматриваем как дополнительные latent operators/effects, а не центральную концепцию.
19. **XS5 / Transport Mixer — hero feature.** Именно на нём объясняем, зачем вообще работать с latent вместо обычного смешивания MP4.
20. **INTERACTION_NET исключаем из 0.1 полностью.** Ни checkpoint, ни зависимость, ни маркетинговые обещания.
21. **Manifold Projector не является realtime-функцией 0.1.** Можно оставить как experimental/offline research tool, но не путать его с основным mixer.
22. **Главная killer-функция — RESAMPLE TO CARTRIDGE:** текущее состояние/процесс можно записать обратно в новый `.lc`.
23. Новый `.lc` после resampling становится **полноценным новым source**, который можно сразу загрузить в любой слот Dual Deck или Quad Deck и смешивать дальше.
24. **LatentPlayer сделать отдельным простым приложением/режимом.** Его задача: открыть `.lc` → Play/Loop/Fullscreen. Это демонстрирует cartridge как самостоятельный media object.
25. **LatentDeck** — synthesis-инструмент. **LatentPlayer** — playback-инструмент. Не смешивать позиционирование.
26. **TAEHV +** **`taeh3`** **остаётся realtime decoder для H3 profile.** Собственный decoder и TAEH3-Super пока не делаем.
27. **Native H3 VAE не нужен обычному пользователю LatentDeck.** Он остаётся исследовательским HQ/reference/authoring инструментом.
28. **Codec architecture должна быть модульной:**
    `LatentDeck Core → Codec Adapter → Decoder`.
    В будущем можно добавить Wan/LTX/другие latent families.
29. **H3 weights не вендорим.** Пользователь получает необходимые H3-compatible decoder assets самостоятельно согласно upstream-инструкции.
30. **Наш GitHub содержит:** Core, `.lc` specification, Player, Deck, mixer algorithms, cartridge SDK, H3 adapter, документацию, примеры создания operators.
31. **H3 cartridges отделяем от GitHub-кода.** Вопрос публичной библиотеки `.lc` и лицензирования конкретных H3 payload решаем отдельно.
32. **Сам** **`.lc`** **standard — наш открытый стандарт**, а права/ограничения конкретного cartridge зависят от его `codec/source/provenance`.
33. **Cartridge SDK обязателен:** validate, inspect, read, write, resample, hash. Люди должны иметь возможность писать свои authoring tools.
34. **Operator API обязателен.** Сообщество должно иметь возможность написать `MyWeirdMixer` без переписывания Deck.
35. **MIDI пока можно не делать ядром релиза**, но control API должен быть независим от UI, чтобы MIDI/OSC добавить без изменения synthesis engine.
36. Я бы всё-таки включил **Spout output** либо поставил его как самый первый post-0.1 milestone. Это позволяет использовать:
    `LatentDeck → Resolume/TouchDesigner/VDMX`
    и избавляет нас от необходимости самим немедленно строить mapping, output routing и прочую VJ-инфраструктуру.
37. **Не делать в 0.1:** audio engine, prompt control, mutant-generative network, timeline, clips/scenes, mapping, projection tools, **больше 4 одновременно активных cartridge-слотов**, production recorder, огромную FX library.
38. **Не заявлять:** «мы изобрели хранение latent». Формулировка должна быть примерно:
    **LatentDeck treats saved generative latents as playable and synthesizable media signals.**
39. **Главная демонстрация релиза:**
    сначала Dual Deck показывает два источника как обычный Linear Mix, затем включается Latent Transport/Hybridize/Interact и становится очевидно, зачем существует latent-domain mixer; затем Quad Deck демонстрирует тот же принцип уже на **четырёх одновременно активных cartridges**.
40. **Главная социальная модель:** один человек может создавать `.lc` на мощной машине, другой — только воспроизводить, микшировать и resample'ить их на consumer GPU.


**четыре самостоятельных, но связанных компонента**, плюс общий стандарт `.lc` и общий Core/API.

Product/Architecture

1. **Название всей экосистемы:** `LatentDeck`.

2. **Главная идея:** LatentDeck рассматривает сохранённые generative latents не как временный технический cache, а как **playable и synthesizable media signals**.

3. Мы **не заявляем**, что изобрели сохранение latent в файл. Новизна проекта — в playback, realtime synthesis, routing, resampling и создании самостоятельной медиасистемы вокруг latent representations.

4. **Название носителя:** `Latent Cartridge`.

5. **Расширение файла:** `.lc`.

6. `.lc` — **наш открытый codec-neutral стандарт**, а не формат MiniMax H3.

7. H3 становится **первым codec profile**, на котором LatentDeck доказан практически.

8. Формат должен допускать будущие профили Wan, LTX и других latent families без изменения базовой спецификации `.lc`.

9. Общая архитектура должна выглядеть так:

   `Latent Cartridge → Codec Adapter → LatentDeck Core → Operators/Mixer → Decoder → Output`.

10. Конкретный `.lc` должен сообщать, каким codec family он создан, например:

`codec_family = minimax_h3`.

11. Cartridge содержит **настоящий latent payload**, а не спрятанный MP4.

12. Для H3 cartridge содержит visual latent и, если доступен, audio latent.

13. **Audio уже входит в cartridge contract 0.1**, но в первом публичном релизе LatentDeck его не проигрывает и не синтезирует.

14. Audio/video interaction переносится в следующую большую версию проекта.

15. Cartridge metadata обязательна: версия `.lc`, codec/profile, tensor schema, dtype, dimensions, decoded resolution, timing/fps, duration, hashes и provenance.

16. Отдельно фиксируются `storage_dtype` и `runtime_dtype`.

17. `.lc` должен поддерживать **genealogy**: `parent_cartridges` и `operation_history`.

18. Это позволяет проследить происхождение resampled cartridge:

`A + B → C`, затем `C + D → E`.

19. Prompt, seed, model/version, LoRA list и исходный workflow/hash желательно сохранять как optional provenance metadata.

20. Prompt при этом **не нужен для playback**. Он сохраняется как история происхождения и потенциальный будущий conditioning source.

---

## 1. LatentDeck Comfy Toolkit

21. Первый компонент экосистемы называется **LatentDeck Comfy Toolkit**.

22. Это официальная **лаборатория и research/development environment** проекта внутри ComfyUI.

23. Именно здесь живёт всё то, что мы сейчас разрабатываем и тестируем.

24. Comfy Toolkit предназначен прежде всего для разработчиков, ML-исследователей, технических художников и людей, создающих новые latent operators.

25. Он **не является главным конечным продуктом для обычного VJ/художника**.

26. В Comfy Toolkit остаются:

* visual latent inspection;
* FAST/HQ comparison;
* XS1–XS5;
* Affinity/Transport/Sinkhorn experiments;
* Manifold Projector;
* новые experimental operators;
* training pipelines;
* InteractionNet/MutantNet R&D;
* codec исследования;
* benchmarking;
* debugging;
* conversion/resampling experiments.

27. Здесь же можно оставлять сложные node workflows, которые не должны попадать в простой standalone UI.

28. Native H3 VAE остаётся именно здесь как **HQ/reference/teacher/authoring tool**.

29. Manifold Projector тоже пока живёт в Toolkit как **offline/research processor**, а не как обязательный realtime-функционал приложения.

30. ComfyUI Core по возможности не патчим. LatentDeck развивается как отдельный набор custom nodes/extensions.

31. Toolkit должен постепенно становиться площадкой, где community сможет создавать и тестировать новые latent operators перед переносом их в standalone LatentDeck.

---

## 2. LatentDeck App

32. Второй компонент — **LatentDeck App**.

33. Это главный standalone audiovisual synthesis instrument проекта.

34. Пользователю LatentDeck App **не требуется ComfyUI**.

35. Пользователь запускает обычное приложение и работает с `.lc`.

36. LatentDeck App не является «обёрткой вокруг Comfy workflow».

37. Текущий Comfy prototype используется как доказанный reference implementation, но production engine должен жить отдельно.

38. Внутри LatentDeck App существует общий **LatentDeck Core**.

39. Core отвечает за:

* cartridge loading;
* codec adapters;
* clock/playback;
* playheads;
* realtime latent processing;
* operator routing;
* decoder management;
* resampling;
* control API;
* output API.

40. Интерфейс конкретной Deck — это не отдельный engine. Разные деки используют один общий Core.

41. В первом публичном релизе LatentDeck App содержит **две виртуальные модели деки**.

42. Первая — условно **LD-D2 / Dual Deck**.

43. Dual Deck содержит два активных cartridge-слота: A и B.

44. Она ориентирована на более глубокое двухисточниковое interaction/cross-synthesis.

45. Dual Deck содержит обычный `Linear A/B` как baseline.

46. Главный latent mixer Dual Deck — нынешняя content-aware линия `Affinity / Sinkhorn Transport`.

47. Основные режимы Dual Deck:

`HYBRIDIZE`
`INTERACT`.

48. Основные controls примерно:

`A/B MIX`
`INTERACTION`
`PRESERVE`
`ROUTING`
`TEMPERATURE`
`CHAOS`.

49. Технические параметры вроде Sinkhorn iterations/top-k прячутся в Advanced.

50. XS1–XS4 остаются как дополнительные latent operators/effects.

51. XS5 / Transport Mixer остаётся **hero feature** первого релиза.

52. Вторая модель — условно **LD-Q4 / Quad Deck**.

53. Quad Deck содержит четыре одновременно активных cartridge-слота: A/B/C/D.

54. Quad Deck использует тот же общий LatentDeck Core и тот же codec system.

55. На старте для Quad не создаём новую ML-архитектуру.

56. Агент должен спроектировать простую deterministic схему распространения уже доказанного latent mixing на четыре sources.

57. Минимально каждый слот должен иметь индивидуальный `LEVEL/WEIGHT`.

58. Итоговые веса должны нормализоваться предсказуемым образом.

59. Желательно иметь **XY performance control**, где четыре cartridges находятся по углам пространства смешивания.

60. XY pad является macro-control над вкладом A/B/C/D, а не единственным способом управления.

61. Quad Deck должна также иметь linear four-source baseline.

62. Затем тот же material направляется в latent-domain processing.

63. Для Quad желательно сохранить общую философию `HYBRIDIZE / INTERACT`.

64. Допускается carrier/donor topology, если она необходима алгоритму.

65. Например один cartridge может выступать structural carrier, а три остальных — donors.

66. Но пользователю должна быть понятна эта логика и возможность сменить carrier.

67. Dual и Quad являются **разными виртуальными аппаратами**, а не просто двумя layouts одного окна.

68. В дальнейшем разные Deck могут иметь собственную processing topology и собственный визуальный характер.

69. Это становится частью design language проекта: **Deck is part of the image**.

70. В будущем возможны другие модели:

* Granular Deck;
* Feedback Deck;
* Text Deck;
* AV Deck;
* experimental community decks.

71. Мы не строим одну гигантскую панель со всеми функциями сразу.

72. Различные Deck должны быть похожи на разные hardware-машины: одна сильна в одном виде synthesis, другая — в другом.

73. Это соответствует эстетике условных Sony/Sharp/Panasonic устройств: общий media standard, но разная машина и разный характер обработки.

---

## 3. Realtime и decoder

74. LatentDeck App работает **в realtime**, а не по схеме `изменил параметр → render clip → play`.

75. Controls воздействуют на следующие latent slots прямо во время текущего playback.

76. Большая generative H3 model в performance loop отсутствует.

77. Для H3 codec profile realtime decode в 0.1 использует текущую проверенную связку:

`StreamingTAEHV + taeh3`.

78. Собственный decoder пока не разрабатываем.

79. TAEH3-Super/Wide пока не является приоритетом.

80. Native H3 VAE обычному пользователю LatentDeck App не нужен.

81. Он остаётся Toolkit/research инструментом.

82. Codec architecture обязательно модульная:

`Core → Codec Adapter → Decoder`.

83. Если позже другой codec окажется лучше H3, LatentDeck не должен потребовать переписывания самого mixer engine.

---

## 4. Resampling

84. **RESAMPLE TO CARTRIDGE — центральная функция LatentDeck App 0.1.**

85. Любое текущее latent synthesis state можно записать обратно в новый `.lc`.

86. В Dual:

`A + B + operators → C.lc`.

87. В Quad:

`A + B + C + D + operators → E.lc`.

88. Новый cartridge можно сразу загрузить обратно в любую Deck.

89. Таким образом LatentDeck поддерживает многопоколенный workflow:

`generation → cartridge → synthesis → cartridge → synthesis → ...`.

90. Именно resampling помогает превратить `.lc` из «сохранённого latent» в полноценный медианоситель и материал композиции.

---

## 5. LatentPlayer App

91. Третий компонент — **LatentPlayer App**.

92. Это отдельное lightweight standalone приложение.

93. Его задача максимально простая:

`.lc → Open → Play / Pause / Loop / Fullscreen`.

94. LatentPlayer не является синтезатором.

95. Его основная концептуальная задача — показать обычному человеку, что `.lc` является самостоятельным playable media object.

96. Для использования LatentPlayer ComfyUI не нужен.

97. LatentPlayer использует тот же Codec Adapter system и decoder infrastructure, что и LatentDeck Core.

98. H3 cartridge требует установленного H3-compatible playback codec.

99. Для H3 это сейчас `StreamingTAEHV + taeh3`.

100. Player не обязан содержать decoder weights внутри нашего собственного distribution.

101. Он может обнаруживать установленный codec либо предлагать инструкции по его установке.

102. Сам LatentPlayer должен оставаться codec-neutral.

103. В дальнейшем один `.lc` можно будет открывать двойным кликом непосредственно в Player.

104. Возможный расширенный `LatentPlayer Mix` пока не входит в обязательный scope.

---

## 6. ComfyUI-LatentCartridge

105. Четвёртый компонент — **`ComfyUI-LatentCartridge`**.

106. Это отдельный маленький пакет custom nodes для **создания исходных `.lc` cartridges**.

107. Его не надо смешивать с большим LatentDeck Comfy Toolkit.

108. Целевая аудитория — человек, который просто генерирует видео в ComfyUI и хочет записать latent cartridge.

109. Основная нода:

**`Save Latent Cartridge (.lc)`**

110. Для H3 она подключается в workflow **до VAE decode**, когда уже получены готовые visual/audio latent.

111. Схема:

`H3 generation → H3 latent → Save Latent Cartridge → .lc`.

112. Пользователь может продолжать получать обычный MP4 параллельно.

113. Recorder не генерирует latent.

114. Он только перехватывает существующее representation и упаковывает его согласно `.lc` specification.

115. В первой версии `ComfyUI-LatentCartridge` фактически содержит H3 recorder/profile.

116. Но архитектура пакета должна сразу предполагать будущие recorder nodes/adapters для Wan/LTX/других моделей.

117. Пользователь в идеале видит одну общую сущность вроде:

`Save Latent Cartridge`

а codec определяется автоматически или выбирается явно.

118. Максимум metadata Recorder получает автоматически.

119. Пользователь не должен вручную вводить H3 channels, temporal compression и прочие параметры, которые можно определить программно.

120. Recorder сохраняет generation provenance, если оно доступно.

121. В частности optional:

prompt, seed, model, LoRA, workflow/hash, дата создания.

122. `ComfyUI-LatentCartridge` должен быть лёгким и не требовать установки полного LatentDeck Toolkit.

123. Человек на мощной генеративной машине должен иметь возможность установить только Recorder nodes.

---

## 7. Converter / Packer

124. Дополнительно нужен небольшой auxiliary tool — **LC Packer/Converter**.

125. Он позволяет превращать уже существующие raw latent `.safetensors` в `.lc`.

126. Это необходимо для старых Continuum/H3 latent files и других архивов.

127. Пример:

`latentdeck-pack input.safetensors --profile h3 -o clip.lc`.

128. Это вспомогательный инструмент, а не основной authoring workflow.

---

## 8. Общая роль четырёх компонентов

129. Итоговая экосистема выглядит так:

**ComfyUI-LatentCartridge**
→ создаёт оригинальные cartridges.

**LatentPlayer App**
→ воспроизводит cartridges.

**LatentDeck App**
→ играет, смешивает, синтезирует и resample'ит cartridges.

**LatentDeck Comfy Toolkit**
→ исследует и создаёт новые способы работать с latent.

130. Другими словами:

**Recorder = студия записи носителя.**

**Player = проигрыватель.**

**Deck = инструмент исполнения.**

**Toolkit = лаборатория разработки.**

---

## 9. Community architecture

131. Сам `.lc` standard должен быть публичным и документированным.

132. **Cartridge SDK обязателен.**

133. Он должен включать:

* read;
* write;
* validate;
* inspect;
* hash;
* resample;
* metadata access.

134. **Operator API обязателен.**

135. Community должна иметь возможность написать собственный `MyWeirdMixer` без переписывания Core.

136. В дальнейшем должен существовать и **Deck API**, чтобы community могла создавать собственные virtual decks/topologies.

137. Codec API также публичный.

138. Новый codec должен определять:

* validation;
* latent layout;
* timing mapping;
* decoder;
* optional encoder;
* compatibility rules.

139. Таким образом community может развивать три независимые культуры:

**Cartridge makers**
**Operator makers**
**Deck makers**.

140. Это одна из центральных социальных моделей проекта.

---

## 10. Output и интеграция с существующими VJ-системами

141. LatentDeck не пытается в 0.1 заменить Resolume/VDMX/TouchDesigner.

142. Он создаёт то, чего у них нет: **latent-domain neural media synthesis**.

143. Затем output можно передать существующему VJ software.

144. Поэтому **Spout output** либо входит в первый публичный релиз, либо является самым первым post-0.1 milestone.

145. Идеальный workflow:

`LatentDeck → Spout → Resolume/TouchDesigner`.

146. Mapping, projection, multi-screen management и прочую зрелую VJ-инфраструктуру пока не повторяем.

---

## 11. Control architecture

147. MIDI не обязан быть частью первой версии.

148. Но control API должен быть независим от UI с самого начала.

149. Тогда позже можно подключить:

* MIDI;
* OSC;
* hardware controllers;
* automation;
* external software.

150. Нельзя зашивать synthesis logic непосредственно в mouse UI controls.

---

## 12. Что исключено из 0.1

151. В публичный 0.1 **не входит** нынешняя `INTERACTION_NET`.

152. Никакого experimental trained checkpoint в обязательном runtime.

153. Mutant-generative network пока не входит.

154. Prompt control не входит.

155. Audio playback/synthesis не входит.

156. Audio→video и video→audio modulation не входит.

157. TAEH3-Super не входит.

158. Production timeline не входит.

159. Clip/scenes manager не входит.

160. Projection mapping не входит.

161. Огромная FX library не входит.

162. Production-grade recorder MP4 не является центральной задачей.

163. Больше четырёх одновременно активных cartridge slots не требуется.

164. Scratch/jog не обещается, пока не решён causal decoder state при arbitrary seek.

165. Manifold Projector остаётся research/offline feature, а не live knob.

---

## 13. Публичное позиционирование

166. Мы не называем LatentDeck «AI video generator».

167. Во время обычного performance новая генерация вообще не происходит.

168. Корректная формула:

**LatentDeck treats saved generative latents as playable and synthesizable media signals.**

169. Или более человечески:

**Generation creates the cartridge. LatentDeck plays the representation.**

170. Первый demo обязательно показывает разницу:

`Linear Mix → Latent Transport → Hybridize/Interact`.

171. Dual Deck демонстрирует глубокое взаимодействие двух representations.

172. Quad Deck демонстрирует более свободное многосоставное synthesis-space.

173. После интересного состояния пользователь нажимает `RESAMPLE` и получает новый cartridge.

174. Это должна быть одна связная демонстрация всей методологии.

---

## 14. Социальная модель

175. Один пользователь с H100/5090/RunPod может заниматься **cartridge authoring**.

176. Другой пользователь с consumer GPU может никогда не запускать H3 Transformer.

177. Он получает `.lc`, проигрывает и синтезирует их локально.

178. Третий пользователь пишет собственные operators.

179. Четвёртый проектирует новую Deck.

180. Пятый собирает коллекции cartridges.

181. В результате generation compute и performance compute становятся отдельными практиками.

---

## 15. H3 и codec independence

182. H3 остаётся первой реально рабочей реализацией LatentDeck.

183. Но `LatentDeck`, `.lc`, Player, SDK и mixer architecture не должны зависеть от названия MiniMax.

184. H3-specific часть помещается в отдельный Codec Profile/Adapter.

185. H3 weights не вендорим в наш основной кодовый repository.

186. Необходимые decoder/model assets пользователь получает самостоятельно согласно upstream-инструкциям.

187. Конкретные H3 `.lc` payloads и их distribution рассматриваются отдельно от лицензии самого LatentDeck Core.

---

## 16. Итоговая структура проекта

188. В концептуальном виде:

```text
LATENTDECK ECOSYSTEM
│
├── Latent Cartridge Standard (.lc)
│
├── LatentPlayer App
│
├── LatentDeck App
│   ├── LD-D2 Dual Deck
│   └── LD-Q4 Quad Deck
│
├── ComfyUI-LatentCartridge
│   ├── H3 Recorder
│   └── future codec recorders
│
├── LatentDeck Comfy Toolkit
│   ├── research
│   ├── operators
│   ├── projector
│   ├── ML experiments
│   └── codec development
│
├── Cartridge SDK
├── Operator API
├── Deck API
└── Codec API
```

189. Самая короткая формула первого публичного релиза:

> **Create → Cartridge → Play → Mix/Synthesize → Resample → Cartridge.**

190. А четыре продукта имеют очень простые роли:

> **ComfyUI-LatentCartridge records it.**
> **LatentPlayer plays it.**
> **LatentDeck performs it.**
> **LatentDeck Comfy Toolkit invents what comes next.**



--------

на чём строим standalone LatentDeck/LatentPlayer и инфраструктуру вокруг них.

1. **Первый публичный desktop target — Windows 11 x64 + NVIDIA.** Не пытаемся одновременно идеально поддержать macOS/Linux/AMD. Архитектуру делаем переносимой, но 0.1 оптимизируем под уже проверенный CUDA/NVIDIA path.

2. **Standalone shell — Tauri 2, не Electron.** Tauri позволяет делать frontend современным web-стеком, а системную часть — на Rust; при этом использует системный WebView вместо упаковки отдельного Chromium. Это даёт нам богатый дизайн/UI без Electron-size/overhead. ([Tauri][1])

3. **Основной системный язык — Rust.** На нём живут LatentDeck Core, lifecycle приложения, cartridge library, scheduler/control state, plugin contracts, output, конфигурация и управление codec workers.

4. **Frontend — Svelte 5 + TypeScript.** Для такого интерфейса я выбрал бы Svelte вместо React: мало boilerplate, очень удобна реактивность для сотен постоянно меняющихся параметров/индикаторов, компилятор генерирует компактный JS. Svelte официально компилирует декларативные компоненты в оптимизированный JavaScript. ([Svelte][2])

5. **Build frontend — Vite.**

6. **CSS/design system — Tailwind CSS 4.x + собственные CSS variables/design tokens.** В 2026 актуальна ветка 4.3; v4 уже ориентирована на современные CSS primitives, CSS variables, P3, container queries и быстрый build. ([Tailwind CSS][3])

7. **Не использовать готовый generic UI-kit как визуальную основу.** Shadcn/Material и подобные можно брать точечно для accessibility primitives, но внешний вид LatentDeck должен быть полностью свой. Нам нужны Sony/Sharp-подобные виртуальные аппараты, а не SaaS dashboard.

8. **Крутилки, фейдеры, XY-pads, waveform/latent scopes — свои Svelte-компоненты.** SVG/Canvas/WebGPU по необходимости. Это позволит каждому Deck иметь собственную faceplate и характер.

9. **UI отделён от realtime engine.** Ни Svelte, ни WebView никогда не должны определять clock, playback или выполнять latent calculations.

10. Архитектура процесса:

`Tauri/Svelte UI ↔ Rust Core ↔ Codec/Compute Worker ↔ GPU`

UI только посылает controls и получает состояние.

11. **Realtime ML/latent compute для первого H3 profile пока остаётся Python + PyTorch.** Не переписываем проверенный pipeline на Rust/C++ только ради архитектурной красоты.

12. Причина: нынешние `TAEHV/taeh3`, Sinkhorn и latent tensor processing уже доказаны именно на PyTorch/CUDA. Переписывание сейчас создаст массу риска и почти нулевой художественный профит.

13. **Python ML живёт отдельным sidecar-процессом**, а не встраивается через PyO3 внутрь основного процесса LatentDeck.

14. Это принципиальное решение. Если codec/torch/CUDA упадёт, **не должна падать вся Deck**. Rust Core перезапускает worker и восстанавливает состояние.

15. H3 worker держит на GPU:
    cartridges A/B или A/B/C/D, operator state, TAEHV decoder и необходимые buffers.

16. **Не гоняем latent tensors туда-сюда между Rust и Python.** Всё latent mixing + decode конкретного codec выполняется внутри одного codec worker. Core передаёт только команды/параметры.

17. **IPC Core ↔ Worker — локальные Named Pipes/Unix-socket abstraction + бинарный MessagePack/CBOR protocol**, не HTTP/REST. REST был удобен для Comfy prototype, но standalone realtime engine он не нужен.

18. Большие frame buffers через control IPC не передаются. Для изображения используем отдельный **shared-memory/ring-buffer transport**.

19. **PyTorch public runtime — pinned stable release + CUDA build.** На сегодня официальный PyTorch предоставляет Windows CUDA 12.8 и CUDA 13.0 builds; для первого публичного Windows package я бы предпочёл **cu128 как более консервативный compatibility target**, а cu130 сделал дополнительным profile. ([PyTorch][4])

20. **TAEHV/taeh3 остаётся H3 realtime decoder 0.1.** Никакой собственной decoder R&D в critical path релиза.

21. При этом Codec Worker API не должен знать слово `taeh3`. Для Core существует абстракция примерно:

`load_codec → load_cartridge → process → decode → seek/reset → unload`.

22. Позже внутри того же API можно заменить PyTorch backend на ONNX Runtime/TensorRT/native implementation без изменения Deck/UI.

23. **TensorRT рассматриваем как optimisation backend, а не как baseline 0.1.** Сначала выпускаем проверенный PyTorch path; затем экспериментируем с экспортом decoder/operator kernels.

24. Для будущей vendor-neutral поддержки можно исследовать **WinML/ONNX Runtime**. DirectML продолжает поддерживаться, но Microsoft сейчас рекомендует WinML как Windows-level abstraction, автоматически выбирающую аппаратный backend. ([ONNX Runtime][5])

25. **Native graphics/presentation layer — Rust + `wgpu`.** В 2026 `wgpu` — зрелый cross-platform Rust GPU API поверх D3D12/Vulkan/Metal; актуальная линия уже v29. ([GitHub][6])

26. Видео **не выводим через JPEG → browser**, как в лаборатории. Это был prototype transport, и наши же тесты показали, что GPU→CPU/presentation был крупнейшим runtime cost, тогда как decoder занимал всего ~1.5 ms.

27. В standalone 0.1 используем:

`CUDA decode → pinned staging buffer/shared memory → wgpu texture → native presentation`.

Этого должно более чем хватить для нынешних 24 fps.

28. **Zero-copy CUDA↔D3D12 shared texture оставить архитектурно возможным, но не делать blocker 0.1.** Это optimisation milestone после стабильного приложения.

29. Причина: при текущем target один CPU/GPU copy уже помещается в realtime budget с огромным запасом. Не надо месяц писать CUDA-D3D interop ради красивой схемы.

30. **Fullscreen/master output рисует native GPU renderer**, а не Tauri WebView.

31. Tauri/WebView отвечает за панели управления. Master output может быть отдельным borderless native render window.

32. Для previews внутри виртуальной Deck допустим lightweight transport в web UI, потому что это маленькие monitor windows, а не master signal.

33. **Spout2 — первый professional video output.** Он предназначен именно для realtime texture sharing в Windows и поддерживает DirectX 9/11/12 и OpenGL. ([GitHub][7])

34. Архитектурная цель:

`LatentDeck native texture → Spout → Resolume/TouchDesigner/etc.`

Желательно без промежуточного encode.

35. NDI не включаем в foundation stack. Это network/video-distribution feature позже.

36. **Realtime scheduler находится в Rust Core**, использует monotonic high-resolution clock, а не browser timers.

37. Decoder/latent clock и presentation clock должны быть разными понятиями. H3 имеет свой temporal contract; renderer имеет 24/30/60 Hz output clock.

38. Нужен bounded ring buffer, frame dropping policy и back-pressure. Никогда не позволяем realtime performance создавать бесконечную очередь кадров.

39. **Cartridge Library index — SQLite.** Файлы `.lc` остаются обычными файлами на диске; SQLite хранит только индекс, metadata, previews, favorites, tags, provenance cache.

40. `.lc` format остаётся отдельной спецификацией от приложения. Версия `.lc` **не равна** версии LatentDeck.

41. Для `.lc` 0.1 я бы ориентировался на простую container-схему:
    `manifest + safetensors payload(s) + optional preview`, без собственного экзотического tensor codec.

42. **Tensor payload — Safetensors**, пока нет веской причины изобретать свой бинарный tensor-format.

43. `.lc` container должен позволять inspect metadata без запуска ML runtime.

44. **Cartridge SDK Core — Rust**, а Python bindings поверх него можно дать через PyO3/maturin для ComfyUI/tooling.

45. Таким образом `.lc` reader/writer/validator не будет иметь две разные несовместимые реализации в App и ComfyUI.

46. `ComfyUI-LatentCartridge` в идеале вызывает **тот же Cartridge SDK**, только через Python bindings.

47. **Operator execution API versioned отдельно.** Например `Operator ABI 0.1`.

48. Для 0.1 встроенные XS/Transport operators всё ещё могут выполняться внутри PyTorch codec worker, потому что там уже находятся CUDA tensors.

49. Не заставляем community сразу писать Rust/CUDA.

50. **Первый community Operator SDK — Python.** Это критично для adoption: ML/VJ-хакер быстрее напишет 30 строк PyTorch, чем Rust+wGPU plugin.

51. Operator получает строго описанный context:
    latent inputs, time slot/window, controls, deterministic seed, codec metadata.

52. Operator не получает прямой доступ ко всему приложению.

53. Позже для быстрых/безопасных operators можно добавить **native/WASM plugin backend**, но не проектируем сложную Wasmtime-платформу до появления реальной потребности.

54. **Deck definition отделяем от operator implementation.** Deck — это topology + controls + allowed operators + UI mapping.

55. Поэтому `LD-D2` и `LD-Q4` в будущем могут описываться частично декларативно, а не быть двумя полностью захардкоженными программами.

56. Но в 0.1 не делаем generic visual node-editor внутри standalone App. ComfyUI уже выполняет роль node-lab.

57. **Configuration/state — serde + versioned JSON/TOML**, без бинарного opaque формата.

58. Preset конкретной Deck должен быть переносимым текстовым документом с version/schema.

59. **Undo/redo — command/state model**, а не копирование всего GPU-state.

60. **Logging — Rust `tracing` + structured logs.** Codec worker также пишет структурированный log со своим process/session ID.

61. Каждый realtime session должен уметь сохранить diagnostic bundle:
    app version, GPU, driver, codec, cartridge hashes, FPS, latency, dropped frames, exception log.

62. Это невероятно важно для open-source community: пользователь должен иметь возможность приложить один diagnostic ZIP к issue.

63. **Crash reporting/telemetry только opt-in.** Никакого скрытого analytics.

64. **Testing stack:**
    Rust — `cargo test` + `cargo-nextest`;
    Python — `pytest`;
    Svelte/TS — `Vitest`;
    UI/end-to-end — `Playwright`.

65. Помимо обычных tests нужны **golden latent tests**: маленькие legal/test cartridges + фиксированные controls → ожидаемые hashes/statistical tolerances.

66. Для GPU operators сохраняем determinism tests отдельно от visual judgement.

67. **Performance regression suite обязателен.** Каждая сборка должна измерять:
    decode time, operator time, frame pacing, VRAM, latency.

68. GitHub-hosted CI не даёт нам нужную RTX-среду, поэтому обычные unit/build tests идут в GitHub Actions, а GPU benchmark — на отдельном/self-hosted NVIDIA runner перед релизом.

69. **Monorepo.** Я бы не делал 12 репозиториев на старте.

70. Пример структуры:

```text
latentdeck/
  apps/
    latentdeck/
    latentplayer/

  crates/
    core/
    cartridge/
    gpu/
    control/
    output-spout/

  codec-host/
    python/
    codecs/
      h3/

  operators/
    builtin/

  comfy/
    toolkit/
    latent-cartridge/

  spec/
    latent-cartridge/
    codec-h3/

  sdk/
  tests/
  docs/
```

71. Rust — Cargo workspace.

72. Frontend — `pnpm` workspace.

73. Python — `uv` для reproducible environments/dependency lock.

74. Все три dependency trees фиксируются lock-файлами:
    `Cargo.lock`, `pnpm-lock.yaml`, `uv.lock`.

75. **Main App installer не содержит H3 weights.** Codec Pack устанавливается отдельно.

76. В интерфейсе нужен **Codec Manager**:

`H3 Playback Codec — installed / missing / incompatible`.

77. Но Codec Manager не должен молча скачивать сторонние веса без отображения upstream/license/source пользователю.

78. H3 Codec Pack содержит runtime dependencies для H3 playback, а не весь H3 generator.

79. Большой H3 Transformer пользователю Player/Deck не нужен вообще.

80. **App update и Codec Pack update — разные механизмы.** Обновление LatentDeck не должно неожиданно заменить decoder/model asset.

81. Версионирование независимое:

`LatentDeck App 0.1.x`
`LC Spec 0.1`
`H3 Codec Profile 0.1`
`H3 Codec Pack x.y`
`Operator API 0.1`.

82. **Windows installer — через Tauri bundler, NSIS/MSI**, с нормальной uninstall/upgrade логикой.

83. Перед реальным публичным распространением — code signing Windows binary.

84. Позже можно добавить WinGet package, но это не blocker первого GitHub release.

85. **SBOM/license audit обязателен для release pipeline.** Cargo/Python/npm зависимости и codec assets должны иметь явное происхождение и лицензию.

86. Модельные веса никогда не должны случайно попасть в Git history или GitHub release assets.

87. **MIDI пока не входит в 0.1 UI**, но когда дойдём до него, на Windows ориентироваться на новый Windows MIDI Services stack. В 2026 он уже включён в актуальные Windows 11 и добавляет MIDI 2.0, multi-client MIDI 1.0 и loopbacks. ([Microsoft GitHub][8])

88. **Audio backend будущего — CPAL как cross-platform abstraction**, с WASAPI/ASIO на Windows. CPAL поддерживает ASIO, JACK, PipeWire и другие backends. ([GitHub][9])

89. Но audio runtime не надо сейчас встраивать «на всякий случай». Просто Core clock/control design не должен предполагать, что видео — единственный поток навсегда.

90. **Не использовать Unity/Unreal/Godot для LatentDeck App.** Нам нужен специализированный media/ML runtime, а не game-engine overhead и чужая модель UI/render lifecycle.

91. **Не строить всё на Electron + Python server.** Для prototype это удобно, для инструмента, который должен жить часами на сцене, я бы выбрал Tauri/Rust + isolated ML worker.

92. **Не переписывать всё сразу на C++.** C++/CUDA extension добавляем только там, где profiler покажет конкретный bottleneck.

93. **Profiler-driven rule:** никакой оптимизации «потому что native быстрее». Наш v1 уже показал хороший урок: предполагали, что bottleneck — decoder, а измерения показали GPU→CPU/presentation.

94. Поэтому сначала архитектурная чистота + измерения, потом TensorRT/CUDA kernels/zero-copy.

95. **Design philosophy UI:** Web technology для максимально свободного внешнего дизайна; native Rust/GPU для всего, где важны latency и стабильность.

96. Это позволяет хоть завтра сделать LD-D2 в стиле серебристого Sony 1987 года, LD-Q4 в стиле лабораторного прибора, а потом полностью сменить skin без прикосновения к realtime engine.

97. **Главная техническая формула standalone системы:**

```text
Svelte 5 UI / Tauri 2
          ↓ controls
       Rust Core
          ↓ IPC
   isolated Codec Worker
    PyTorch + CUDA
          ↓
 latent mixer + TAEHV
          ↓ frames
 native wgpu renderer
          ↓
   Window / Spout
```

98. **LatentPlayer использует тот же stack**, только без operator/mixer subsystem:

```text
Tauri/Svelte
    ↓
Rust Core
    ↓
Codec Worker
    ↓
wgpu
```

99. **LatentDeck Comfy Toolkit и ComfyUI-LatentCartridge остаются Python/ComfyUI-проектами.** Не надо насильно переносить их на Rust — это research/authoring environment, где Python является преимуществом.

100. И главный архитектурный принцип, который я бы буквально написал первой строкой технического документа:

> **The UI is disposable. Decks are replaceable. Codecs are replaceable. Cartridges and the realtime signal contract are the stable center of LatentDeck.**

Если сокращать всё это до выбора стека: **Tauri 2 + Svelte 5/TypeScript + Tailwind 4.3 для оболочки; Rust + Tokio + wgpu для Core/realtime/presentation; isolated Python/PyTorch CUDA codec worker для neural compute; Safetensors внутри `.lc`; SQLite для библиотеки; Spout2 для VJ-output; GitHub Actions + self-hosted RTX runner для CI/performance.**

Я бы именно этот комплект принимал за baseline для Technical Design Document 0.1.

[1]: https://v2.tauri.app/ "Tauri 2.0 | Tauri"
[2]: https://svelte.dev/docs/svelte/overview "Overview • Svelte Docs"
[3]: https://tailwindcss.com/blog/tailwindcss-v4?utm_source=chatgpt.com "Tailwind CSS v4.0 - Tailwind CSS"
[4]: https://pytorch.org/get-started/previous-versions/?utm_source=chatgpt.com "Previous PyTorch Versions"
[5]: https://onnxruntime.ai/docs/get-started/with-windows.html?utm_source=chatgpt.com "Windows | onnxruntime"
[6]: https://github.com/gfx-rs/wgpu?utm_source=chatgpt.com "GitHub - gfx-rs/wgpu: A cross-platform, safe, pure-Rust graphics API. · GitHub"
[7]: https://github.com/leadedge/Spout2?utm_source=chatgpt.com "GitHub - leadedge/Spout2: A video frame sharing system for Microsoft Windows · GitHub"
[8]: https://microsoft.github.io/MIDI/?utm_source=chatgpt.com "About Windows MIDI Services - Windows MIDI Services"
[9]: https://github.com/RustAudio/cpal?utm_source=chatgpt.com "GitHub - RustAudio/cpal: Cross-platform audio I/O library in pure Rust · GitHub"

--------

## 17. Закрывающие решения перед стартом работы

191. **Compatibility contract 0.1.** В рамках одного active synthesis session cartridges должны быть совместимы по `codec profile`, `latent geometry` и `timing contract`.

192. Для первого публичного релиза LatentDeck **не выполняет скрытые conversion/re-encode/resize операции** между несовместимыми cartridges.

193. Если cartridges несовместимы, Deck должен **явно сообщать об этом пользователю**.

194. Более сложные compatibility/conversion pipelines относятся к будущим версиям и не входят в 0.1.

195. **Точка RESAMPLE жёстко зафиксирована.** `RESAMPLE TO CARTRIDGE` записывает **post-operator latent state до decoder**, а не RGB-output после decode.

196. Иными словами, pipeline выглядит так:

197. `cartridges → latent synthesis / operators → RESAMPLE .lc → decoder`.

198. Это принципиально важно: новый `.lc` должен оставаться **настоящим latent cartridge**, а не контейнером с уже декодированным видео.

199. **Топология LD-Q4 / Quad Deck для 0.1 зафиксирована как `carrier + 3 donors`.**

200. Один cartridge выступает **Carrier** и задаёт основную structural основу / continuity результата.

201. Три других cartridges выступают **Donors** и воздействуют на Carrier через общий latent interaction pipeline.

202. Эта схема выбрана как стартовая для 0.1, потому что она наиболее понятна, управляемая и соответствует художественной задаче первого публичного релиза.

203. Пользователь должен иметь возможность **явно выбирать, какой слот является Carrier**.

204. Donor-slots B/C/D должны иметь **независимый influence control**.

205. Дополнительно допускается общий performance control-поле/XY/Influence Field для удобного управления относительным вкладом Donors.

206. Полностью симметричный 4-way mode можно исследовать позже, но **в 0.1 он не является обязательным**.

207. **Формат `.lc` и cartridge loading рассматриваются как работа с недоверенным пользовательским media-файлом.**

208. Loader обязан выполнять **строгую schema validation** и проверку manifest/payload структуры.

209. Loader обязан валидировать `dtype`, tensor shapes, sizes, codec/profile compatibility и разумные memory limits до передачи данных в runtime.

210. `.lc` должен оставаться **data-only format**.

211. Внутри `.lc` не допускаются исполняемые скрипты, произвольные команды или любые механизмы, способные выполнять код при открытии cartridge.

212. Ошибочный или повреждённый cartridge не должен ломать приложение: корректное поведение — безопасный reject с понятным сообщением пользователю.

213. Эти ограничения являются частью базового security/stability contract LatentDeck 0.1.
