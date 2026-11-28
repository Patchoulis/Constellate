# Third-party notices

## Tabler Icons

The app icon (`apps/web/app/icon.svg`, `apps/desktop/icon.png`) and the mark in the sidebar
(`apps/web/components/brand.tsx`) use the paths of the "universe" icon from Tabler Icons,
https://github.com/tabler/tabler-icons, recoloured and placed on a background tile.

MIT License

Copyright (c) 2020-2026 Paweł Kuna

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

## Other icons

Interface icons come from Lucide (https://lucide.dev, ISC licence), installed as the `lucide-react` package.

## The engine's runtime libraries

The engine (`resources/engine`) has ONNX Runtime (https://github.com/microsoft/onnxruntime, MIT licence,
Copyright (c) Microsoft Corporation) compiled in, as built by the `ort` crate's project (pyke.io), and next to it:

- on Windows, `DirectML.dll`: DirectML (https://github.com/microsoft/DirectML), Copyright (c) Microsoft
  Corporation, redistributed unmodified under the licence Microsoft publishes for its redistributable package;
- on Linux, `libwebgpu_dawn.so`: Dawn (https://dawn.googlesource.com/dawn), the WebGPU implementation,
  BSD 3-Clause licence, Copyright the Dawn and Chromium authors.

## Glyph names and TeX font layouts

The engine reads what a PDF's fonts draw by their glyph names, through the Adobe Glyph List For New Fonts
(https://github.com/adobe-type-tools/agl-aglfn), compiled into it as a table (`apps/engine/src/text/pdf/fonts/glyphs.rs`):

Copyright 2002-2019 Adobe (http://www.adobe.com/).

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
following conditions are met:

Redistributions of source code must retain the above copyright notice, this list of conditions and the following
disclaimer.

Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following
disclaimer in the documentation and/or other materials provided with the distribution.

Neither the name of Adobe nor the names of its contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Which symbol each slot of TeX's math fonts holds (`apps/engine/src/text/pdf/fonts/tex.rs`) is taken from the fonts'
own declarations: Knuth's Computer Modern (cmsy, cmmi, cmex) as LaTeX declares them (`fontmath.ltx`, `omsenc.def`,
`omlenc.def`, `omxenc.def`), and Anthony Phan's mathabx (`mathabx.dcl`, `mathxdrv.mf`, `mathacnt.mf`). The ToUnicode
maps and CFF font programs are parsed with the crates `adobe-cmap-parser` (MIT) and `cff-parser` (MIT or Apache-2.0),
the ones `pdf-extract` uses.

## pdf.js

The Text tab draws a paper's figures from its PDF with pdf.js (https://github.com/mozilla/pdf.js), the `pdfjs-dist`
package, version 6.5.438, run in a sandboxed frame of its own (`apps/web/lib/reader/figure-frame.ts`). It is
Copyright Mozilla Foundation and its contributors, under the Apache License, Version 2.0
(https://www.apache.org/licenses/LICENSE-2.0); its licence text is installed with the app with the other packages'.

## The embedding model

The model the app downloads at its first run, Qwen3-Embedding-0.6B in the ONNX form published by onnx-community on
Hugging Face, is not part of the packages. It is distributed by its authors under the Apache-2.0 licence.

## In the packages

The installed app also contains the packages its interface runs on (Next.js, React and their dependencies, KaTeX,
d3, Radix UI, Lucide), the Rust crates compiled into the engine, Electron and Chromium. Their licence texts are
installed with the app, in `resources/licenses` (the crates in `resources/licenses/rust`, with a table of every
crate, its version and licence, made from Cargo's own account of the build) and, for Electron and Chromium, next to
the program.
