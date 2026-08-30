"""LatentDeck Comfy Toolkit public Python surface."""

from .adapter import (
    MAX_SEQUENCE_VALUES,
    MAX_TEMPORAL_SLOTS,
    TOOLKIT_ADAPTER_VERSION,
    XsSequenceResult,
    process_xs_sequence,
)
from .decoder_compare import (
    MAX_DECODED_VALUES,
    MAX_DECODER_INPUT_VALUES,
    MAX_METRIC_CHUNK_VALUES,
    DecoderComparison,
    DecoderHook,
    ToolkitContractError,
    compare_decoder_hooks,
)
from .nodes import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS
from .operator_api import (
    MAX_OPERATOR_PROVENANCE_BYTES,
    OPERATOR_API_VERSION,
    ExternalOperatorDescriptor,
    InstalledOperator,
    OperatorContext,
    ToolkitOperatorResult,
    TrustedOperatorRegistry,
    get_operator_descriptor_schema,
    validate_external_descriptor,
)
from .projector import (
    MAX_PROJECTOR_TOKENS,
    PROJECTOR_VERSION,
    ProjectionResult,
    preflight_projector_input,
    project_offline,
)
from .research_nodes import (
    LatentDeckResearchOperatorHook,
    build_installed_operator_research_hook,
)

__version__ = "0.1.0"

__all__ = [
    "MAX_SEQUENCE_VALUES",
    "MAX_TEMPORAL_SLOTS",
    "MAX_DECODED_VALUES",
    "MAX_DECODER_INPUT_VALUES",
    "MAX_METRIC_CHUNK_VALUES",
    "MAX_OPERATOR_PROVENANCE_BYTES",
    "MAX_PROJECTOR_TOKENS",
    "NODE_CLASS_MAPPINGS",
    "NODE_DISPLAY_NAME_MAPPINGS",
    "DecoderComparison",
    "DecoderHook",
    "ExternalOperatorDescriptor",
    "InstalledOperator",
    "LatentDeckResearchOperatorHook",
    "OPERATOR_API_VERSION",
    "OperatorContext",
    "PROJECTOR_VERSION",
    "ProjectionResult",
    "TOOLKIT_ADAPTER_VERSION",
    "ToolkitContractError",
    "ToolkitOperatorResult",
    "TrustedOperatorRegistry",
    "XsSequenceResult",
    "__version__",
    "compare_decoder_hooks",
    "build_installed_operator_research_hook",
    "get_operator_descriptor_schema",
    "project_offline",
    "preflight_projector_input",
    "validate_external_descriptor",
    "process_xs_sequence",
]
