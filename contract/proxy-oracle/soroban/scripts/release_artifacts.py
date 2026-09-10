"""The proxy-oracle Soroban release artifacts, shared by the manifest writer and dry-run validator."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Artifact:
    manifest_key: str
    install_key: str
    package: str

    @property
    def optimized_wasm(self) -> str:
        return f"{self.package.replace('-', '_')}.optimized.wasm"


ARTIFACTS = (
    Artifact("runtime_wasm", "install_runtime", "templar-proxy-oracle-soroban-contract"),
    Artifact("governance_wasm", "install_governance", "templar-proxy-oracle-soroban-governance-contract"),
    Artifact("sep40_adapter_wasm", "install_sep40_adapter", "templar-proxy-oracle-soroban-sep40-adapter-contract"),
    Artifact("lazer_source_wasm", "install_lazer_source", "templar-proxy-oracle-soroban-pyth-lazer-source-contract"),
    Artifact("batcher_wasm", "install_batcher", "templar-proxy-oracle-soroban-batcher-contract"),
)
