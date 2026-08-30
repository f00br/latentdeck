# LatentDeck third-party notices

LatentDeck's original source and documentation are licensed under Apache-2.0.
The following notices cover third-party source used directly by the v0.1
applications and H3 Codec Pack. Transitive package inventory is emitted by
`tools/New-Sbom.ps1` from the committed lock files.

## Tiny AutoEncoder for Hunyuan Video (`taehv`)

- Source: <https://github.com/madebyollin/taehv>
- Pinned commit: `e743234f3217ab3d1570f65642ab06596d1bd7c5`
- Copyright: 2025 Ollin Boer Bohan
- License: MIT

LatentDeck's H3 Codec Pack includes a source-text-identical, LF-normalized copy
of `taehv.py`. Its detailed provenance and hashes are stored with the adapter
under `codec-host/codecs/h3/src/latentdeck_codec_h3/_vendor/`. No TAEH3 model
weight is distributed.

```text
MIT License

Copyright (c) 2025 Ollin Boer Bohan

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Spout2

- Source: <https://github.com/leadedge/Spout2>
- Tag: `2.007.017`
- Pinned commit: `f49e2f469f8cb25f559a6eaa61a3f5b8173fc100`
- Copyright: 2020-2024 Lynn Jarvis
- License: BSD-2-Clause

Spout2 source is not vendored in Git. The release build prepares and verifies
the exact upstream archive locally, then statically links the native sender
libraries. LatentDeck's C ABI bridge is original project code.

```text
BSD 2-Clause License

Copyright (c) 2020-2024, Lynn Jarvis
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
