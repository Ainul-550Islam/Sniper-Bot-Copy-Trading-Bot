// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { KillSwitchPanel } from "../components/operator/KillSwitchPanel";
import { ModeSwitcher } from "../components/operator/ModeSwitcher";
import { ModuleToggleTable } from "../components/operator/ModuleToggleTable";
import { KeyList } from "../components/operator/KeyList";

const killSystemMock = vi.fn();
const setModeMock = vi.fn();
const disableModuleMock = vi.fn();
const addKeyMock = vi.fn();

vi.mock("@/lib/api/operator-api", async () => {
  const actual = await vi.importActual<Record<string, unknown>>("@/lib/api/operator-api");
  return {
    ...actual,
    killSystem: () => killSystemMock(),
    setMode: (mode: string) => setModeMock(mode),
    disableModule: (name: string) => disableModuleMock(name),
    addKey: (input: unknown) => addKeyMock(input),
  };
});

describe("operator console components", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  describe("KillSwitchPanel typed confirmation", () => {
    it("requires typed confirm before engaging global kill switch", async () => {
      killSystemMock.mockResolvedValue({ ok: true, kill_switch: true });
      render(<KillSwitchPanel killSwitchActive={false} />);

      // Click the emergency stop button
      fireEvent.click(screen.getByRole("button", { name: /ENGAGE EMERGENCY STOP/i }));

      // Typed confirmation input is visible
      const confirmInput = screen.getByLabelText(/Type EMERGENCY STOP to confirm/i);
      expect(confirmInput).toBeTruthy();

      const confirmButton = screen.getByRole("button", { name: /KILL SYSTEM NOW/i });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(true);

      // Typing wrong text keeps button disabled
      fireEvent.change(confirmInput, { target: { value: "WRONG" } });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(true);
      expect(killSystemMock).not.toHaveBeenCalled();

      // Typing exact phrase enables the button
      fireEvent.change(confirmInput, { target: { value: "EMERGENCY STOP" } });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(false);

      // Clicking calls the kill system endpoint
      fireEvent.click(confirmButton);
      await waitFor(() => {
        expect(killSystemMock).toHaveBeenCalledTimes(1);
      });
    });
  });

  describe("ModeSwitcher typed confirmation", () => {
    it("requires typed confirm before switching to live mode", async () => {
      setModeMock.mockResolvedValue({ ok: true, mode: "live", live_allowed: true });
      render(<ModeSwitcher currentMode="paper" />);

      // Click live trading button
      fireEvent.click(screen.getByRole("button", { name: /Live Trading/i }));

      // Typed confirmation input appears
      const confirmInput = screen.getByLabelText(/Type GO LIVE to confirm/i);
      expect(confirmInput).toBeTruthy();

      const confirmButton = screen.getByRole("button", { name: /Confirm Live Mode/i });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(true);

      // Typing partial or wrong text keeps disabled
      fireEvent.change(confirmInput, { target: { value: "GO" } });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(true);
      expect(setModeMock).not.toHaveBeenCalled();

      // Typing exact phrase enables
      fireEvent.change(confirmInput, { target: { value: "GO LIVE" } });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(false);

      fireEvent.click(confirmButton);
      await waitFor(() => {
        expect(setModeMock).toHaveBeenCalledWith("live");
      });
    });
  });

  describe("ModuleToggleTable typed confirmation", () => {
    it("requires typed confirm before disabling a running module", async () => {
      disableModuleMock.mockResolvedValue({ ok: true, module: "sniper", enabled: false });
      const modules = [
        {
          module: "sniper",
          enabled: true,
          running: true,
          healthy: true,
          connected: true,
          events_seen: 100,
          signals_generated: 10,
          orders_sent: 5,
          orders_filled: 4,
          orders_failed: 1,
          orders_rejected_by_risk: 0,
          realized_pnl: 12.5,
          unrealized_pnl: 0,
          consecutive_errors: 0,
        },
      ];

      render(<ModuleToggleTable modules={modules} onRefresh={() => {}} />);

      // Click Disable
      fireEvent.click(screen.getByRole("button", { name: /Disable/i }));

      // Typed confirm appears
      const confirmInput = screen.getByLabelText(/Type DISABLE SNIPER to confirm/i);
      expect(confirmInput).toBeTruthy();

      const confirmButton = screen.getByRole("button", { name: /Confirm disable/i });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(true);

      fireEvent.change(confirmInput, { target: { value: "DISABLE SNIPER" } });
      expect((confirmButton as HTMLButtonElement).disabled).toBe(false);

      fireEvent.click(confirmButton);
      await waitFor(() => {
        expect(disableModuleMock).toHaveBeenCalledWith("sniper");
      });
    });
  });

  describe("KeyList security & secret redaction", () => {
    it("never displays secret material and clears secret input field on submission", async () => {
      addKeyMock.mockResolvedValue({
        ok: true,
        label: "ci-key",
        role: "operator",
        hash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      });

      const existingKeys = [
        {
          label: "production-deployer",
          role: "operator",
          hash: "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e",
          source: "database",
          enabled: true,
          created_at: new Date().toISOString(),
        },
      ];

      render(<KeyList keys={existingKeys} onRefresh={() => {}} />);

      // Asserts existing keys table displays label and truncated hash, NEVER plaintext secret
      expect(screen.getByText("production-deployer")).toBeTruthy();
      expect(screen.getByText(/a591a6d40bf42040/)).toBeTruthy();

      // Form input for secret must be type=password
      const secretInput = screen.getByLabelText(/Key Secret/i) as HTMLInputElement;
      expect(secretInput.type).toBe("password");

      const labelInput = screen.getByLabelText(/Key Label/i);
      fireEvent.change(labelInput, { target: { value: "ci-key" } });
      const testSecret = "super-secret-key-material-min-24-chars";
      fireEvent.change(secretInput, { target: { value: testSecret } });

      expect(secretInput.value).toBe(testSecret);

      const submitButton = screen.getByRole("button", { name: /Register Key/i });
      fireEvent.click(submitButton);

      await waitFor(() => {
        expect(addKeyMock).toHaveBeenCalledWith({
          label: "ci-key",
          key: testSecret,
          role: "operator",
        });
      });

      // Assert secret field was cleared immediately
      expect(secretInput.value).toBe("");
      // Secret text is never in DOM
      expect(screen.queryByText(testSecret)).toBeNull();
    });
  });
});
