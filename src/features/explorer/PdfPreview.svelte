<script lang="ts">
  import ChevronLeftIcon from "@lucide/svelte/icons/chevron-left";
  import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";
  import Maximize2Icon from "@lucide/svelte/icons/maximize-2";
  import MinusIcon from "@lucide/svelte/icons/minus";
  import PanelLeftIcon from "@lucide/svelte/icons/panel-left";
  import PlusIcon from "@lucide/svelte/icons/plus";
  import type {
    PDFDocumentLoadingTask,
    PDFDocumentProxy,
    RenderTask,
  } from "pdfjs-dist/legacy/build/pdf.mjs";
  // Tauri follows each OS's system WebView. PDF.js's maintained legacy build
  // supplies proposal-stage collection methods that those engines may not yet
  // expose, while keeping the parser and worker on the pinned PDF.js version.
  import pdfWorkerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
  import { tick } from "svelte";
  import type { Action } from "svelte/action";
  import { SvelteMap, SvelteSet } from "svelte/reactivity";

  import { Button } from "$lib/components/ui/button";

  const MAX_PAGES = 500;
  const MAX_CANVAS_PIXELS = 16_000_000;
  const MAX_DEVICE_PIXEL_RATIO = 2;
  const LOAD_TIMEOUT_MS = 10_000;
  const RENDER_TIMEOUT_MS = 5_000;
  const MIN_ZOOM = 50;
  const MAX_ZOOM = 200;
  const ZOOM_STEP = 25;
  const DEFAULT_PAGE_RATIO = 8.5 / 11;

  let { data, title }: { data: ArrayBuffer; title: string } = $props();

  let root = $state<HTMLDivElement>();
  let documentScroller = $state<HTMLDivElement>();
  let thumbnailScroller = $state<HTMLDivElement>();
  let loading = $state(true);
  let errorMessage = $state<string | null>(null);
  let pages = $state<number[]>([]);
  let currentPage = $state(1);
  let zoomPercent = $state(100);
  let thumbnailOpen = $state(true);
  let thumbnailPreference = $state<boolean | null>(null);
  let pageRatios = $state<Record<number, number>>({});
  let pageErrors = $state<number[]>([]);

  let loadingTask: PDFDocumentLoadingTask | null = null;
  let documentProxy: PDFDocumentProxy | null = null;
  let pdfjs: typeof import("pdfjs-dist/legacy/build/pdf.mjs") | null = null;
  let lifecycle = 0;
  let pageObserver: IntersectionObserver | null = null;
  let thumbnailObserver: IntersectionObserver | null = null;
  let resizeFrame: number | null = null;

  const pageElements = new SvelteMap<number, HTMLElement>();
  const thumbnailElements = new SvelteMap<number, HTMLElement>();
  const pageCanvases = new SvelteMap<number, HTMLCanvasElement>();
  const thumbnailCanvases = new SvelteMap<number, HTMLCanvasElement>();
  const visiblePageRatios = new SvelteMap<number, number>();
  const visibleThumbnails = new SvelteSet<number>();
  const pageTasks = new SvelteMap<number, RenderTask>();
  const thumbnailTasks = new SvelteMap<number, RenderTask>();
  const pageQueue: number[] = [];
  const thumbnailQueue: number[] = [];
  const queuedPages = new SvelteSet<number>();
  const queuedThumbnails = new SvelteSet<number>();
  const renderedPages = new SvelteSet<number>();
  const renderedThumbnails = new SvelteSet<number>();
  let activePageRenders = 0;
  let activeThumbnailRenders = 0;

  const pdfAssetUrl = (group: string) =>
    `${import.meta.env.BASE_URL}pdfjs/${group}/`;

  const desiredPages = () => {
    const desired = new SvelteSet<number>();
    for (const page of visiblePageRatios.keys()) {
      for (let candidate = page - 1; candidate <= page + 1; candidate += 1) {
        if (candidate >= 1 && candidate <= pages.length) desired.add(candidate);
      }
    }
    return desired;
  };

  const clearCanvas = (canvas: HTMLCanvasElement | undefined) => {
    if (!canvas) return;
    canvas.width = 0;
    canvas.height = 0;
    canvas.style.width = "";
    canvas.style.height = "";
  };

  const cancelPageRender = (page: number) => {
    pageTasks.get(page)?.cancel();
    pageTasks.delete(page);
    queuedPages.delete(page);
    renderedPages.delete(page);
    const index = pageQueue.indexOf(page);
    if (index >= 0) pageQueue.splice(index, 1);
    clearCanvas(pageCanvases.get(page));
  };

  const cancelThumbnailRender = (page: number) => {
    thumbnailTasks.get(page)?.cancel();
    thumbnailTasks.delete(page);
    queuedThumbnails.delete(page);
    renderedThumbnails.delete(page);
    const index = thumbnailQueue.indexOf(page);
    if (index >= 0) thumbnailQueue.splice(index, 1);
    clearCanvas(thumbnailCanvases.get(page));
  };

  const updateCurrentPage = () => {
    let nextPage = currentPage;
    let largestRatio = -1;
    for (const [page, ratio] of visiblePageRatios) {
      if (ratio > largestRatio) {
        nextPage = page;
        largestRatio = ratio;
      }
    }
    if (largestRatio >= 0) currentPage = nextPage;
  };

  const updateDesiredPageRenders = () => {
    const desired = desiredPages();
    for (const page of pageTasks.keys()) {
      if (!desired.has(page)) cancelPageRender(page);
    }
    for (const page of queuedPages) {
      if (!desired.has(page)) cancelPageRender(page);
    }
    for (const page of desired) queuePageRender(page);
  };

  const queuePageRender = (page: number) => {
    if (
      !documentProxy ||
      pageTasks.has(page) ||
      queuedPages.has(page) ||
      renderedPages.has(page) ||
      !pageCanvases.has(page)
    )
      return;
    queuedPages.add(page);
    pageQueue.push(page);
    pumpPageQueue();
  };

  const queueThumbnailRender = (page: number) => {
    if (
      !documentProxy ||
      thumbnailTasks.has(page) ||
      queuedThumbnails.has(page) ||
      renderedThumbnails.has(page) ||
      !thumbnailCanvases.has(page)
    )
      return;
    queuedThumbnails.add(page);
    thumbnailQueue.push(page);
    pumpThumbnailQueue();
  };

  const pumpPageQueue = () => {
    while (activePageRenders < 2 && pageQueue.length > 0) {
      const page = pageQueue.shift();
      if (page === undefined) return;
      queuedPages.delete(page);
      if (!desiredPages().has(page)) continue;
      activePageRenders += 1;
      void renderPage(page, false).finally(() => {
        activePageRenders -= 1;
        pumpPageQueue();
      });
    }
  };

  const pumpThumbnailQueue = () => {
    while (activeThumbnailRenders < 2 && thumbnailQueue.length > 0) {
      const page = thumbnailQueue.shift();
      if (page === undefined) return;
      queuedThumbnails.delete(page);
      if (!visibleThumbnails.has(page)) continue;
      activeThumbnailRenders += 1;
      void renderPage(page, true).finally(() => {
        activeThumbnailRenders -= 1;
        pumpThumbnailQueue();
      });
    }
  };

  const renderPage = async (pageNumber: number, thumbnail: boolean) => {
    const token = lifecycle;
    const pdf = documentProxy;
    const canvas = thumbnail
      ? thumbnailCanvases.get(pageNumber)
      : pageCanvases.get(pageNumber);
    if (!pdf || !canvas) return;

    let renderTask: RenderTask | null = null;
    let timedOut = false;
    try {
      const page = await pdf.getPage(pageNumber);
      if (token !== lifecycle) return;
      if (thumbnail && !visibleThumbnails.has(pageNumber)) return;
      if (!thumbnail && !desiredPages().has(pageNumber)) return;

      const baseViewport = page.getViewport({ scale: 1 });
      const ratio = baseViewport.width / baseViewport.height;
      if (!thumbnail && pageRatios[pageNumber] !== ratio) {
        pageRatios = { ...pageRatios, [pageNumber]: ratio };
      }

      const containerWidth = thumbnail
        ? 88
        : Math.max(
            240,
            (pageElements.get(pageNumber)?.clientWidth ?? 720) - 32,
          );
      const cssScale = containerWidth / baseViewport.width;
      const cssViewport = page.getViewport({ scale: cssScale });
      let outputScale = Math.min(
        window.devicePixelRatio || 1,
        MAX_DEVICE_PIXEL_RATIO,
      );
      const scaledPixels =
        cssViewport.width * outputScale * cssViewport.height * outputScale;
      if (scaledPixels > MAX_CANVAS_PIXELS) {
        outputScale *= Math.sqrt(MAX_CANVAS_PIXELS / scaledPixels);
      }

      const renderViewport = page.getViewport({
        scale: cssScale * outputScale,
      });
      canvas.width = Math.max(1, Math.floor(renderViewport.width));
      canvas.height = Math.max(1, Math.floor(renderViewport.height));
      canvas.style.width = `${Math.floor(cssViewport.width)}px`;
      canvas.style.height = `${Math.floor(cssViewport.height)}px`;
      const context = canvas.getContext("2d", { alpha: false });
      if (!context) throw new Error("Canvas unavailable");

      renderTask = page.render({
        canvas,
        canvasContext: context,
        viewport: renderViewport,
        annotationMode: pdfjs?.AnnotationMode.DISABLE ?? 0,
        background: "#ffffff",
      });
      (thumbnail ? thumbnailTasks : pageTasks).set(pageNumber, renderTask);
      const timeout = window.setTimeout(() => {
        timedOut = true;
        renderTask?.cancel();
      }, RENDER_TIMEOUT_MS);
      try {
        await renderTask.promise;
      } finally {
        window.clearTimeout(timeout);
      }
      (thumbnail ? renderedThumbnails : renderedPages).add(pageNumber);
      if (!thumbnail && pageErrors.includes(pageNumber)) {
        pageErrors = pageErrors.filter((page) => page !== pageNumber);
      }
    } catch (error) {
      const cancelled =
        error instanceof Error && error.name === "RenderingCancelledException";
      if (!thumbnail && (!cancelled || timedOut) && token === lifecycle) {
        pageErrors = [...new SvelteSet([...pageErrors, pageNumber])];
      }
    } finally {
      if (renderTask) {
        (thumbnail ? thumbnailTasks : pageTasks).delete(pageNumber);
      }
    }
  };

  const rerenderVisiblePages = () => {
    for (const page of new SvelteSet([
      ...pageTasks.keys(),
      ...queuedPages,
      ...renderedPages,
    ])) {
      cancelPageRender(page);
    }
    updateDesiredPageRenders();
  };

  const changeZoom = (next: number) => {
    zoomPercent = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, next));
    void tick().then(rerenderVisiblePages);
  };

  const scrollToPage = (page: number) => {
    const next = Math.min(pages.length, Math.max(1, page));
    pageElements.get(next)?.scrollIntoView({
      behavior: "smooth",
      block: "start",
    });
  };

  const handleDocumentKeydown = (event: KeyboardEvent) => {
    if (event.key === "Home") {
      event.preventDefault();
      scrollToPage(1);
    } else if (event.key === "End") {
      event.preventDefault();
      scrollToPage(pages.length);
    }
  };

  const toggleThumbnails = () => {
    thumbnailPreference = !thumbnailOpen;
    thumbnailOpen = !thumbnailOpen;
  };

  const observePage: Action<HTMLElement, number> = (node, page) => {
    pageElements.set(page, node);
    pageObserver?.observe(node);
    queueMicrotask(() => updateDesiredPageRenders());
    return {
      destroy() {
        pageObserver?.unobserve(node);
        pageElements.delete(page);
        visiblePageRatios.delete(page);
        cancelPageRender(page);
      },
    };
  };

  const observeThumbnail: Action<HTMLElement, number> = (node, page) => {
    thumbnailElements.set(page, node);
    thumbnailObserver?.observe(node);
    return {
      destroy() {
        thumbnailObserver?.unobserve(node);
        thumbnailElements.delete(page);
        visibleThumbnails.delete(page);
        cancelThumbnailRender(page);
      },
    };
  };

  const registerPageCanvas: Action<HTMLCanvasElement, number> = (
    node,
    page,
  ) => {
    pageCanvases.set(page, node);
    queueMicrotask(() => queuePageRender(page));
    return {
      destroy() {
        pageCanvases.delete(page);
        cancelPageRender(page);
      },
    };
  };

  const registerThumbnailCanvas: Action<HTMLCanvasElement, number> = (
    node,
    page,
  ) => {
    thumbnailCanvases.set(page, node);
    queueMicrotask(() => queueThumbnailRender(page));
    return {
      destroy() {
        thumbnailCanvases.delete(page);
        cancelThumbnailRender(page);
      },
    };
  };

  const initializeObservers = () => {
    pageObserver?.disconnect();
    thumbnailObserver?.disconnect();
    pageObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const page = Number((entry.target as HTMLElement).dataset.page);
          if (entry.isIntersecting) {
            visiblePageRatios.set(page, entry.intersectionRatio);
          } else {
            visiblePageRatios.delete(page);
          }
        }
        updateCurrentPage();
        updateDesiredPageRenders();
      },
      { root: documentScroller, threshold: [0, 0.25, 0.5, 0.75, 1] },
    );
    thumbnailObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const page = Number((entry.target as HTMLElement).dataset.page);
          if (entry.isIntersecting) {
            visibleThumbnails.add(page);
            queueThumbnailRender(page);
          } else {
            visibleThumbnails.delete(page);
            cancelThumbnailRender(page);
          }
        }
      },
      { root: thumbnailScroller, rootMargin: "120px" },
    );
  };

  const loadPdf = async (source: ArrayBuffer) => {
    const token = ++lifecycle;
    loading = true;
    errorMessage = null;
    pages = [];
    currentPage = 1;
    pageRatios = {};
    pageErrors = [];

    let task: PDFDocumentLoadingTask;
    try {
      pdfjs ??= await import("pdfjs-dist/legacy/build/pdf.mjs");
      if (token !== lifecycle) return;
      pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;
      task = pdfjs.getDocument({
        data: new Uint8Array(source.slice(0)),
        cMapUrl: pdfAssetUrl("cmaps"),
        cMapPacked: true,
        iccUrl: pdfAssetUrl("iccs"),
        standardFontDataUrl: pdfAssetUrl("standard_fonts"),
        wasmUrl: pdfAssetUrl("wasm"),
        useWorkerFetch: true,
        useSystemFonts: true,
        enableXfa: false,
        stopAtErrors: true,
        maxImageSize: MAX_CANVAS_PIXELS,
      });
    } catch {
      if (token === lifecycle) {
        errorMessage = "This PDF couldn’t be displayed.";
        loading = false;
      }
      return;
    }
    loadingTask = task;
    let passwordProtected = false;
    task.onPassword = () => {
      passwordProtected = true;
      if (token === lifecycle) {
        errorMessage = "Password-protected PDFs aren’t supported yet.";
        loading = false;
      }
      void task.destroy();
    };
    const timeout = window.setTimeout(() => {
      if (token === lifecycle) {
        errorMessage = "This PDF couldn’t be displayed.";
        loading = false;
      }
      void task.destroy();
    }, LOAD_TIMEOUT_MS);

    try {
      const pdf = await task.promise;
      window.clearTimeout(timeout);
      if (token !== lifecycle) {
        await task.destroy();
        return;
      }
      if (pdf.numPages > MAX_PAGES) {
        errorMessage = "This PDF has too many pages to preview.";
        loading = false;
        await task.destroy();
        return;
      }
      documentProxy = pdf;
      pages = Array.from({ length: pdf.numPages }, (_, index) => index + 1);
      loading = false;
      requestAnimationFrame(() => {
        if (token !== lifecycle) return;
        initializeObservers();
        for (const element of pageElements.values())
          pageObserver?.observe(element);
        for (const element of thumbnailElements.values())
          thumbnailObserver?.observe(element);
      });
    } catch (error) {
      window.clearTimeout(timeout);
      if (token !== lifecycle || passwordProtected) return;
      if (error instanceof Error && error.name === "PasswordException") {
        errorMessage = "Password-protected PDFs aren’t supported yet.";
      } else {
        errorMessage = "This PDF couldn’t be displayed.";
      }
      loading = false;
    }
  };

  const teardown = () => {
    lifecycle += 1;
    pageObserver?.disconnect();
    thumbnailObserver?.disconnect();
    pageObserver = null;
    thumbnailObserver = null;
    for (const task of pageTasks.values()) task.cancel();
    for (const task of thumbnailTasks.values()) task.cancel();
    pageTasks.clear();
    thumbnailTasks.clear();
    pageQueue.length = 0;
    thumbnailQueue.length = 0;
    queuedPages.clear();
    queuedThumbnails.clear();
    renderedPages.clear();
    renderedThumbnails.clear();
    visiblePageRatios.clear();
    visibleThumbnails.clear();
    pageElements.clear();
    thumbnailElements.clear();
    pageCanvases.clear();
    thumbnailCanvases.clear();
    const task = loadingTask;
    loadingTask = null;
    documentProxy = null;
    if (task && !task.destroyed) void task.destroy();
  };

  $effect(() => {
    const source = data;
    void loadPdf(source);
    return teardown;
  });

  $effect(() => {
    if (!root || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      if (thumbnailPreference === null) {
        thumbnailOpen = entry.contentRect.width >= 900;
      }
    });
    observer.observe(root);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (!documentScroller || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (resizeFrame !== null) cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(() => {
        resizeFrame = null;
        rerenderVisiblePages();
      });
    });
    observer.observe(documentScroller);
    return () => {
      observer.disconnect();
      if (resizeFrame !== null) cancelAnimationFrame(resizeFrame);
    };
  });
</script>

<div bind:this={root} class="relative flex size-full min-h-0 bg-muted/70">
  {#if loading}
    <div class="grid size-full place-items-center" aria-busy="true">
      <LoaderCircleIcon class="size-5 animate-spin text-muted-foreground" />
    </div>
  {:else if errorMessage}
    <div
      class="grid size-full place-items-center p-8 text-center"
      role="status"
    >
      <p class="max-w-sm text-sm text-muted-foreground">{errorMessage}</p>
    </div>
  {:else}
    <aside
      class:hidden={!thumbnailOpen}
      class="w-32 shrink-0 border-r bg-background"
      aria-label="PDF pages"
    >
      <div
        bind:this={thumbnailScroller}
        class="h-full overflow-y-auto px-3 py-4"
      >
        <div class="space-y-3">
          {#each pages as page (page)}
            <button
              type="button"
              data-page={page}
              use:observeThumbnail={page}
              class="group block w-full rounded-md text-center outline-none"
              aria-label={`Go to page ${page}`}
              aria-current={currentPage === page ? "page" : undefined}
              onclick={() => scrollToPage(page)}
            >
              <span
                class:border-foreground={currentPage === page}
                class="grid min-h-28 place-items-center overflow-hidden rounded-sm border bg-white shadow-sm transition-colors group-hover:border-foreground/50 group-focus-visible:ring-2 group-focus-visible:ring-ring"
              >
                <canvas
                  use:registerThumbnailCanvas={page}
                  class="block max-w-full"
                  aria-hidden="true"
                ></canvas>
              </span>
              <span class="mt-1 block text-[10px] text-muted-foreground">
                {page}
              </span>
            </button>
          {/each}
        </div>
      </div>
    </aside>

    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      bind:this={documentScroller}
      data-preview-document
      role="application"
      aria-label={`PDF preview of ${title}`}
      tabindex="0"
      onkeydown={handleDocumentKeydown}
      class="min-w-0 flex-1 overflow-auto px-4 pt-6 pb-24 outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset sm:px-8"
    >
      <div class="mx-auto flex w-full max-w-4xl flex-col items-start gap-5">
        {#each pages as page (page)}
          <section
            data-page={page}
            use:observePage={page}
            aria-label={`Page ${page}`}
            class="mx-auto grid shrink-0 scroll-mt-6 place-items-center"
            style:aspect-ratio={pageRatios[page] ?? DEFAULT_PAGE_RATIO}
            style:width={`${zoomPercent}%`}
          >
            {#if pageErrors.includes(page)}
              <div
                class="grid size-full min-h-80 place-items-center border bg-white text-sm text-muted-foreground shadow-sm"
              >
                Page couldn’t be displayed.
              </div>
            {:else}
              <canvas
                use:registerPageCanvas={page}
                class="block bg-white shadow-md ring-1 ring-black/10"
                aria-hidden="true"
              ></canvas>
            {/if}
          </section>
        {/each}
      </div>
    </div>

    <div
      class="pointer-events-none absolute inset-x-0 bottom-5 z-10 flex justify-center px-4"
    >
      <div
        class="pointer-events-auto flex items-center gap-0.5 rounded-lg border bg-background p-1 shadow-lg"
        role="toolbar"
        aria-label="PDF controls"
      >
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={thumbnailOpen
            ? "Hide page thumbnails"
            : "Show page thumbnails"}
          title={thumbnailOpen
            ? "Hide page thumbnails"
            : "Show page thumbnails"}
          aria-pressed={thumbnailOpen}
          onclick={toggleThumbnails}
        >
          <PanelLeftIcon />
        </Button>
        <span class="mx-1 h-4 w-px bg-border" aria-hidden="true"></span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Previous page"
          title="Previous page"
          disabled={currentPage <= 1}
          onclick={() => scrollToPage(currentPage - 1)}
        >
          <ChevronLeftIcon />
        </Button>
        <span class="min-w-14 text-center text-xs tabular-nums">
          {currentPage} / {pages.length}
        </span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Next page"
          title="Next page"
          disabled={currentPage >= pages.length}
          onclick={() => scrollToPage(currentPage + 1)}
        >
          <ChevronRightIcon />
        </Button>
        <span class="mx-1 h-4 w-px bg-border" aria-hidden="true"></span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Zoom out"
          title="Zoom out"
          disabled={zoomPercent <= MIN_ZOOM}
          onclick={() => changeZoom(zoomPercent - ZOOM_STEP)}
        >
          <MinusIcon />
        </Button>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Fit width"
          title="Fit width"
          onclick={() => changeZoom(100)}
        >
          <Maximize2Icon />
        </Button>
        <span class="min-w-10 text-center text-xs tabular-nums">
          {zoomPercent}%
        </span>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Zoom in"
          title="Zoom in"
          disabled={zoomPercent >= MAX_ZOOM}
          onclick={() => changeZoom(zoomPercent + ZOOM_STEP)}
        >
          <PlusIcon />
        </Button>
      </div>
    </div>
  {/if}
</div>
