# LATENTDECK — CONCEPT DOCUMENT

## 1. Что такое LatentDeck

**LatentDeck — это система для воспроизведения, смешивания и синтеза сохранённых latent-представлений генеративного видео в реальном времени.**

Проект исходит из простой идеи:

> результат генеративной модели не обязан заканчиваться декодированным MP4.

До превращения в обычные пиксели существует latent representation — компактное машинное представление видео, содержащее пространственную, временную и другую learned-информацию.

LatentDeck рассматривает этот latent **не как временный технический артефакт генерации, а как самостоятельный playable media signal**.

Главная формула проекта:

**Generation creates the cartridge. LatentDeck plays the representation.**

---

## 2. Latent Cartridge

Сохранённый playable latent называется **Latent Cartridge**.

Расширение:

`.lc`

Cartridge — это не MP4 и не контейнер, внутри которого спрятано обычное видео.

Он содержит настоящий latent payload и описание того, каким codec/model family этот latent создан и как его следует интерпретировать.

H3 является первым практически работающим codec profile, но сам стандарт `.lc` не принадлежит H3 и должен оставаться независимым от конкретной модели.

В будущем один и тот же LatentDeck должен потенциально уметь работать с другими latent families через отдельные codec adapters.

---

## 3. Почему это не просто другой видеоплеер

Обычный video workflow:

`generation → RGB/video → MP4 → video mixer`

После декодирования VJ работает уже с пикселями.

LatentDeck:

`generation → latent cartridge → latent synthesis → decoder → image`

Следовательно, LatentDeck может изменять representation **до возникновения конечного изображения**.

В этом находится основное отличие проекта от Resolume, VDMX и обычного shader/video processing.

LatentDeck не пытается заменить эти программы.

Он создаёт дополнительный тип source/synthesizer, output которого позже может быть передан в существующую VJ-среду.

---

## 4. Что значит «синтезировать latent»

Самая простая операция — обычное смешивание двух representations.

Она нужна как baseline, но не является основной причиной существования проекта.

Главный интерес начинается там, где информация cartridges взаимодействует на уровне latent features.

Первый доказанный подход проекта использует content-aware **Affinity / Sinkhorn Transport**: representation одного cartridge анализируется относительно другого, после чего признаки между ними маршрутизируются и взаимодействуют до decoder.

Основные художественные режимы:

**HYBRIDIZE** — representations стремятся образовать общую смешанную материю.

**INTERACT** — один материал структурно воздействует на другой.

XS1–XS4 существуют как дополнительные latent-processing techniques; Transport Mixer является центральным методом первого релиза.

LatentDeck не обещает, что два изображения автоматически породят семантически осмысленный «третий объект».

Текущая цель более фундаментальна:

> дать художнику новый controllable domain обработки видео, недоступный после окончательного RGB decode.

---

## 5. Realtime является частью самого определения

LatentDeck — не интерфейс для очередной генерации.

Во время normal performance большая generative model не работает.

Cartridges уже созданы.

Они загружаются, проигрываются и проходят через latent operators непосредственно во время исполнения.

Изменение ручки должно воздействовать на проходящий сейчас latent stream, а не запускать:

`change parameter → render file → playback`.

Поэтому LatentDeck является **инструментом исполнения**, а generation pipeline является отдельной стадией создания материала.

---

## 6. Resampling

Одна из фундаментальных операций LatentDeck — **RESAMPLE TO CARTRIDGE**.

Результат latent synthesis можно снова записать как новый `.lc`.

Например:

`A + B → C.lc`

или:

`A + B + C + D → E.lc`.

Новый cartridge ничем принципиально не отличается от исходного и может снова быть загружен в Deck.

Так возникает многопоколенный workflow:

**generation → cartridge → synthesis → cartridge → synthesis → ...**

Это важная часть философии проекта.

Cartridge — не только воспроизводимый файл, но и сырьё для создания следующего поколения cartridges.

---

# 7. Deck как виртуальная машина

LatentDeck не должен превращаться в одну гигантскую панель со всеми возможными функциями.

Вместо этого существуют разные **Deck** — виртуальные аппараты с разной topology и характером.

Это ближе к физическим синтезаторам, видеомагнитофонам и специализированным media devices.

**Deck is part of the image.**

Один и тот же cartridge может вести себя по-разному в разных Deck.

Первый релиз содержит две модели.

### LD-D2 — Dual Deck

Два cartridges: A и B.

Основное назначение — глубокое двухисточниковое latent interaction.

### LD-Q4 — Quad Deck

Четыре cartridges.

Начальная topology:

**1 Carrier + 3 Donors.**

Carrier задаёт основную structure/continuity, а три Donors в разной степени воздействуют на него.

Это не просто «четыре слоя opacity», а четырёхисточниковый latent synthesizer.

В будущем могут появляться совсем другие машины:

* Granular Deck;
* Feedback Deck;
* Text Deck;
* AV Deck;
* community-created Decks.

---

# 8. Четыре части экосистемы

LatentDeck состоит не из одной программы.

### LatentDeck App

Основной standalone инструмент художника.

Он проигрывает, смешивает, синтезирует и resample'ит cartridges.

ComfyUI пользователю не требуется.

### LatentPlayer App

Простой standalone player:

`.lc → Play / Pause / Loop / Fullscreen`.

Его задача — сделать cartridge воспринимаемым как настоящий самостоятельный media object, а не как ML-файл для программиста.

### ComfyUI-LatentCartridge

Небольшой authoring toolkit.

Пользователь, генерирующий видео в ComfyUI, подключает Recorder к latent output и получает `.lc`.

Для первой версии это в первую очередь H3 recorder, но сама идея должна распространяться и на будущие codec families.

### LatentDeck Comfy Toolkit

Research laboratory проекта.

Здесь создаются новые operators, исследуются decoder'ы, manifold projection, ML approaches, новые codecs и будущие формы synthesis.

То, что доказало ценность в лаборатории, позднее может стать новой Deck или новым operator в standalone-приложении.

---

# 9. Что мы строим для сообщества

LatentDeck должен быть не только законченной программой, но и **открытой средой для новой практики работы с latent media**.

Предполагаются несколько типов пользователей:

**Cartridge makers** создают и распространяют `.lc`.

**Performers** используют готовые cartridges без необходимости запускать большую generative model.

**Operator makers** придумывают новые способы latent processing.

**Deck makers** создают новые virtual machines и topology.

**Codec developers** добавляют поддержку новых latent families.

Именно поэтому Cartridge API, Operator API, Deck API и Codec API являются важной частью архитектуры.

---

# 10. Чего LatentDeck 0.1 намеренно НЕ делает

Первый релиз не должен пытаться стать полноценной DAW, Resolume или media server.

Пока исключены:

* audio playback/synthesis;
* audio/video interaction;
* prompt control;
* generative mutant network;
* production timeline;
* scenes/clips system;
* projection mapping;
* огромная библиотека FX;
* scratch/jog;
* больше четырёх одновременно активных cartridges.

Эти функции могут появиться позже, но они не должны размывать первый публичный тезис проекта.

---

# 11. Что должен доказать первый публичный релиз

Человек должен суметь пройти очень простую цепочку:

**CREATE**

создать `.lc` из generative workflow;

↓

**PLAY**

открыть cartridge как самостоятельный media object;

↓

**SYNTHESIZE**

загрузить несколько cartridges в LatentDeck и воздействовать на их latent representations в realtime;

↓

**RESAMPLE**

записать результат как новый cartridge;

↓

**PLAY AGAIN**

использовать получившийся `.lc` как новый исходный материал.

Короткая формула:

> **Create → Cartridge → Play → Mix/Synthesize → Resample → Cartridge.**

Если разработчик понимает эту цепочку, он понимает проект.

---

# 12. Главный принцип для разработчика

LatentDeck нельзя незаметно превратить в обычный видеомикшер с необычным форматом входного файла.

Любое важное архитектурное решение следует проверять вопросом:

> **Используем ли мы здесь свойства latent representation, которые исчезли бы или стали недоступны после превращения источника в обычный MP4?**

Если ответ постоянно становится «нет», значит проект уходит от своей основной идеи.

LatentDeck существует потому, что **latent становится медиумом исполнения**, а не потому, что нам нужен ещё один способ показать видео.

И главный архитектурный принцип:

> **The UI is disposable. Decks are replaceable. Codecs are replaceable. Cartridges and the realtime signal contract are the stable center of LatentDeck.**
