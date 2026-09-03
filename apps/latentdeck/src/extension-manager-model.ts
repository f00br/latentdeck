export type ExtensionPackageKind = "deck_pack" | "codec_pack";
export type ExtensionPackageHealth =
  "healthy" | "verification_required" | "corrupt" | "untrusted";

export type ExtensionCompatibilityReason =
  | "compatible"
  | "untrusted"
  | "missing_asset"
  | "package_invalid"
  | "unsupported_protocol"
  | "unsupported_host_api"
  | "unsupported_tensor_abi"
  | "unsupported_profile"
  | "unsupported_signal"
  | "unsupported_timing"
  | "unsupported_capability";

export interface ExtensionPackageReference {
  kind: ExtensionPackageKind;
  packageId: string;
  packageVersion: string;
}

export interface ExtensionPackageSummary {
  package: ExtensionPackageReference;
  displayName: string | null;
  publisherName: string | null;
  enabled: boolean;
  health: ExtensionPackageHealth;
  errorCode: string | null;
  errorDetail: string | null;
}

export interface ExtensionProfileKey {
  codecFamily: string;
  profile: string;
  profileVersion: string;
}

export interface ExtensionCompatibilityPair {
  deck: ExtensionPackageReference;
  codec: ExtensionPackageReference;
  reason: ExtensionCompatibilityReason;
  compatibleProfiles: ExtensionProfileKey[];
  compatibleProfile: ExtensionProfileKey | null;
}

export interface ExtensionsSnapshot {
  packages: ExtensionPackageSummary[];
  matrix: ExtensionCompatibilityPair[];
}

export interface InspectedExtension {
  package: ExtensionPackageReference;
  displayName: string;
  publisherName: string;
  publisherIdentityClaim: "self_declared";
  archiveSha256: string;
  archiveByteLength: number;
  fileCount: number;
  extractedByteLength: number;
}

export const EMPTY_EXTENSIONS_SNAPSHOT: ExtensionsSnapshot = {
  packages: [],
  matrix: [],
};

const COMPATIBILITY_LABELS: Record<ExtensionCompatibilityReason, string> = {
  compatible: "Compatible",
  untrusted: "Package is not trusted",
  missing_asset: "Required external asset missing",
  package_invalid: "Package is invalid",
  unsupported_protocol: "Unsupported worker protocol",
  unsupported_host_api: "Unsupported host API",
  unsupported_tensor_abi: "Unsupported tensor ABI",
  unsupported_profile: "Unsupported codec profile",
  unsupported_signal: "Unsupported signal geometry",
  unsupported_timing: "Unsupported timing",
  unsupported_capability: "Required capability unavailable",
};

export function extensionPackageKey(
  packageReference: ExtensionPackageReference,
): string {
  return [
    packageReference.kind,
    packageReference.packageId,
    packageReference.packageVersion,
  ].join("\u0000");
}

export function isExactLowercaseSha256(value: string): boolean {
  return /^[0-9a-f]{64}$/.test(value);
}

export function shaConfirmationMatches(
  userValue: string,
  measuredSha256: string,
): boolean {
  return isExactLowercaseSha256(userValue) && userValue === measuredSha256;
}

export function inspectionMatchesPackage(
  inspection: InspectedExtension,
  packageReference: ExtensionPackageReference,
): boolean {
  return (
    inspection.package.kind === packageReference.kind &&
    inspection.package.packageId === packageReference.packageId &&
    inspection.package.packageVersion === packageReference.packageVersion
  );
}

export function publisherIdentityNotice(
  inspection: Pick<InspectedExtension, "publisherName">,
): string {
  return `${inspection.publisherName} · self-declared metadata; SHA-256 confirms bytes, not publisher identity.`;
}

export function compatibilityReasonLabel(
  reason: ExtensionCompatibilityReason,
): string {
  return COMPATIBILITY_LABELS[reason];
}

export function replaceVerifiedSummary(
  snapshot: ExtensionsSnapshot,
  verified: ExtensionPackageSummary,
): ExtensionsSnapshot {
  const verifiedKey = extensionPackageKey(verified.package);
  return {
    ...snapshot,
    packages: snapshot.packages.map((summary) =>
      extensionPackageKey(summary.package) === verifiedKey ? verified : summary,
    ),
  };
}
