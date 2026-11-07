// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ createStrategy: vi.fn() }));
vi.mock("@/lib/api/strategy-api", () => ({ createStrategy: (...args: unknown[]) => mocks.createStrategy(...args) }));

import StrategyForm from "@/components/strategy/strategy-form";

describe("strategy form parameter object validation", () => {
  beforeEach(() => mocks.createStrategy.mockReset().mockResolvedValue({ id: "strategy-1" }));
  afterEach(() => cleanup());

  it.each(["null", "[]", "1", "\"text\""])('rejects valid JSON that is not an object: %s', async (raw) => {
    render(<StrategyForm onSuccess={vi.fn()} onCancel={vi.fn()} />);
    const textboxes = screen.getAllByRole("textbox");
    fireEvent.change(textboxes[0]!, { target: { value: "Test strategy" } });
    fireEvent.change(textboxes[2]!, { target: { value: raw } });
    fireEvent.click(screen.getByRole("button", { name: "Save Strategy" }));

    expect(await screen.findByText("Parameters must be a JSON object")).toBeTruthy();
    expect(mocks.createStrategy).not.toHaveBeenCalled();
  });

  it("submits a JSON object as the strategy parameters", async () => {
    const onSuccess = vi.fn();
    render(<StrategyForm onSuccess={onSuccess} onCancel={vi.fn()} />);
    const textboxes = screen.getAllByRole("textbox");
    fireEvent.change(textboxes[0]!, { target: { value: "Test strategy" } });
    fireEvent.change(textboxes[2]!, { target: { value: "{\"max_buy_sol\": 2}" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Strategy" }));

    await waitFor(() => expect(mocks.createStrategy).toHaveBeenCalledWith(expect.objectContaining({
      name: "Test strategy",
      parameters: { max_buy_sol: 2 },
    })));
    expect(onSuccess).toHaveBeenCalledTimes(1);
  });
});
