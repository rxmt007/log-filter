import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(() => cleanup());

class TestResizeObserver implements ResizeObserver {
  static instances = new Set<TestResizeObserver>();

  readonly observed = new Set<Element>();

  constructor(private readonly callback: ResizeObserverCallback) {
    TestResizeObserver.instances.add(this);
  }

  observe(target: Element) {
    this.observed.add(target);
  }

  unobserve(target: Element) {
    this.observed.delete(target);
  }

  disconnect() {
    this.observed.clear();
    TestResizeObserver.instances.delete(this);
  }

  emit(target: Element, width: number, height: number) {
    if (!this.observed.has(target)) return;
    const rect = {
      x: 0,
      y: 0,
      top: 0,
      right: width,
      bottom: height,
      left: 0,
      width,
      height,
      toJSON: () => ({}),
    };
    this.callback(
      [
        {
          target,
          contentRect: rect,
          borderBoxSize: [{ inlineSize: width, blockSize: height }],
          contentBoxSize: [{ inlineSize: width, blockSize: height }],
          devicePixelContentBoxSize: [{ inlineSize: width, blockSize: height }],
        } as ResizeObserverEntry,
      ],
      this,
    );
  }
}

Object.defineProperty(globalThis, "ResizeObserver", {
  configurable: true,
  writable: true,
  value: TestResizeObserver,
});

export { TestResizeObserver };
