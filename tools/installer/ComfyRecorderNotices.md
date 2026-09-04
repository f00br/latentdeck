# Third-party notices

The LatentDeck Comfy LC Recorder bundle includes `safetensors` 0.8.0 from
<https://pypi.org/project/safetensors/0.8.0/> under Apache-2.0. Its license is
included at `licenses/safetensors-LICENSE`.

The bundled Safetensors Python package is relocated under
`latentdeck_recorder_vendor`, and its internal absolute import is rewritten to
a relative import solely for namespace isolation.

The `latentdeck-cartridge` and `latentdeck-comfy-cartridge` wheels are
LatentDeck project components released under the repository's Apache-2.0
license. The bundle contains no model weights, decoder assets, cartridges, or
ComfyUI distribution.
