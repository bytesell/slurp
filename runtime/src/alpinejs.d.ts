/**
 * Minimal type declarations for alpinejs v3.
 *
 * Alpine ships without bundled `.d.ts` files. We declare only the surface
 * area this package actually uses so TypeScript can type-check without
 * falling back to `any`.
 */
declare module 'alpinejs' {
  /** An Alpine component data factory. */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  type AlpineDataFactory = (el: HTMLElement) => Record<string, any>

  const Alpine: {
    /**
     * Starts Alpine - processes all `x-data` components in the DOM.
     * Call once after registering plugins and data factories.
     */
    start(): void

    /**
     * Initialises Alpine on a subtree. Used after client-side DOM swaps.
     * @param el - Root element of the subtree to initialise.
     */
    initTree(el: HTMLElement): void

    /**
     * Registers a named data factory. Components can reference it in
     * `x-data="name(args)"`.
     *
     * @param name - The component name.
     * @param factory - Factory function returning the component data object.
     */
    data(name: string, factory: AlpineDataFactory): void
  }

  export default Alpine
}
