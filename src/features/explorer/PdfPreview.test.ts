import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createDemoPdf } from "$lib/data/demo-pdf";

const pdfMocks = vi.hoisted(() => ({
  destroy: vi.fn(async () => {}),
  getDocument: vi.fn(),
  render: vi.fn(() => ({
    promise: Promise.resolve(),
    cancel: vi.fn(),
  })),
}));

vi.mock("pdfjs-dist/legacy/build/pdf.mjs", () => ({
  AnnotationMode: { DISABLE: 0 },
  GlobalWorkerOptions: { workerSrc: "" },
  getDocument: pdfMocks.getDocument,
}));

import PdfPreview from "./PdfPreview.svelte";

const defaultResizeObserver = window.ResizeObserver;

const documentProxy = {
  numPages: 3,
  getPage: vi.fn(async () => ({
    getViewport: ({ scale }: { scale: number }) => ({
      width: 612 * scale,
      height: 792 * scale,
    }),
    render: pdfMocks.render,
  })),
};

const loadingTask = () => ({
  destroyed: false,
  onPassword: () => {},
  promise: Promise.resolve(documentProxy),
  destroy: pdfMocks.destroy,
});

class ImmediateIntersectionObserver {
  private readonly callback: IntersectionObserverCallback;

  constructor(callback: IntersectionObserverCallback) {
    this.callback = callback;
  }

  observe(target: Element) {
    this.callback(
      [
        {
          target,
          isIntersecting: true,
          intersectionRatio:
            Number((target as HTMLElement).dataset.page) === 1 ? 1 : 0.25,
        } as IntersectionObserverEntry,
      ],
      this as unknown as IntersectionObserver,
    );
  }

  disconnect() {}
  unobserve() {}
  takeRecords() {
    return [];
  }
  root = null;
  rootMargin = "0px";
  thresholds = [0];
}

class WideResizeObserver {
  private readonly callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
  }

  observe(target: Element) {
    this.callback(
      [
        {
          target,
          contentRect: { width: 1_024 } as DOMRectReadOnly,
        } as ResizeObserverEntry,
      ],
      this as unknown as ResizeObserver,
    );
  }

  disconnect() {}
  unobserve() {}
}

beforeEach(() => {
  pdfMocks.destroy.mockClear();
  pdfMocks.getDocument.mockReset();
  pdfMocks.render.mockClear();
  pdfMocks.getDocument.mockReturnValue(loadingTask());
  vi.stubGlobal("IntersectionObserver", ImmediateIntersectionObserver);
  window.ResizeObserver =
    WideResizeObserver as unknown as typeof ResizeObserver;
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
    {} as CanvasRenderingContext2D,
  );
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  window.ResizeObserver = defaultResizeObserver;
});

describe("PdfPreview", () => {
  it("renders continuous pages with responsive thumbnails and zoom controls", async () => {
    const view = render(PdfPreview, {
      data: createDemoPdf(),
      title: "handoff.pdf",
    });

    expect(
      await screen.findByRole("application", {
        name: "PDF preview of handoff.pdf",
      }),
    ).toBeInTheDocument();
    expect(screen.getByText("1 / 3")).toBeInTheDocument();
    expect(
      screen.getByRole("complementary", { name: "PDF pages" }),
    ).toBeVisible();

    await fireEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(screen.getByText("125%")).toBeInTheDocument();
    await fireEvent.click(
      screen.getByRole("button", { name: "Hide page thumbnails" }),
    );
    expect(
      screen.getByRole("complementary", { name: "PDF pages" }),
    ).toHaveClass("hidden");

    await waitFor(() => expect(pdfMocks.render).toHaveBeenCalled());
    view.unmount();
    await waitFor(() => expect(pdfMocks.destroy).toHaveBeenCalled());
  });

  it("shows a concise state for password-protected PDFs", async () => {
    pdfMocks.getDocument.mockImplementationOnce(() => {
      const task = {
        destroyed: false,
        onPassword: () => {},
        promise: new Promise(() => {}),
        destroy: pdfMocks.destroy,
      };
      queueMicrotask(() => task.onPassword());
      return task;
    });

    render(PdfPreview, {
      data: createDemoPdf(),
      title: "locked.pdf",
    });

    expect(
      await screen.findByText("Password-protected PDFs aren’t supported yet."),
    ).toBeInTheDocument();
  });
});
