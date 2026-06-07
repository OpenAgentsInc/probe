import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import {
  assertProbePublicProjection,
  canIssueProviderAccountGrant,
  CHATGPT_CODEX_PROVIDER,
  sanitizeProbePublicProjection,
  validateProbePublicProjection,
  type PublicProviderAccount,
} from "../src";

const connectedAccount = (overrides: Partial<PublicProviderAccount> = {}): PublicProviderAccount => ({
  provider: CHATGPT_CODEX_PROVIDER,
  providerAccountRef: "provider-account_primary" as PublicProviderAccount["providerAccountRef"],
  authMode: "chatgpt_device_code",
  status: "connected",
  health: "healthy",
  secretRef: "codex-auth://provider-account_primary" as PublicProviderAccount["secretRef"],
  accountLabel: "Primary ChatGPT",
  planType: "plus",
  ...overrides,
});

describe("Probe/Omega provider account contract", () => {
  test("supports multiple explicitly connected ChatGPT/Codex accounts", () => {
    const accounts = [
      connectedAccount(),
      connectedAccount({
        providerAccountRef: "provider-account_backup" as PublicProviderAccount["providerAccountRef"],
        secretRef: "codex-auth://provider-account_backup" as PublicProviderAccount["secretRef"],
        accountLabel: "Backup ChatGPT",
      }),
    ];

    expect(accounts).toHaveLength(2);
    expect(accounts.every(canIssueProviderAccountGrant)).toBe(true);
    expect(accounts.map((account) => account.providerAccountRef)).toEqual([
      "provider-account_primary",
      "provider-account_backup",
    ]);
  });

  test("blocks grant issuance for disconnected, unhealthy, or secretless accounts", () => {
    expect(canIssueProviderAccountGrant(connectedAccount({ status: "disconnected" }))).toBe(false);
    expect(canIssueProviderAccountGrant(connectedAccount({ health: "requires_reauth" }))).toBe(false);
    expect(canIssueProviderAccountGrant(connectedAccount({ secretRef: undefined }))).toBe(false);
  });

  test("allows public secret refs but rejects raw credential material through Effect validation", async () => {
    await expect(Effect.runPromise(validateProbePublicProjection(connectedAccount()))).resolves.toBeUndefined();

    await expect(
      Effect.runPromise(
        validateProbePublicProjection({
          providerSecretRef: "sk-proj_123456789012345678901234567890",
        }),
      ),
    ).rejects.toMatchObject({ _tag: "ProbePublicProjectionUnsafe" });

    await expect(
      Effect.runPromise(
        validateProbePublicProjection({
          metadata: {
            access_token: "raw-token",
          },
        }),
      ),
    ).rejects.toMatchObject({
      _tag: "ProbePublicProjectionUnsafe",
      path: "projection.metadata.access_token",
    });

    expect(() =>
      assertProbePublicProjection({
        metadata: {
          access_token: "raw-token",
        },
      }),
    ).toThrow();
  });

  test("sanitizes logs and receipts while preserving public refs", () => {
    const sanitized = sanitizeProbePublicProjection({
      providerSecretRef: "codex-auth://provider-account_primary",
      message: "Bearer very-secret-access-token-value-1234567890",
      nested: {
        refresh_token: "raw-refresh-token",
      },
    });

    expect(sanitized).toEqual({
      providerSecretRef: "codex-auth://provider-account_primary",
      message: "[redacted]",
      nested: {
        refresh_token: "[redacted]",
      },
    });
  });
});
