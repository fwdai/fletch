// Rendering a hook or a component in a test, for what only a render shows: a
// store selector that never settles passes every test of the code around it
// and still takes its component down with React error #185.
//
// A file that renders needs a DOM, which the default environment has not:
// start it with `// @vitest-environment happy-dom`.
//
// Import from here, not from Testing Library: it unmounts what a test rendered
// only where the test functions are globals, and they are not in this project.
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(cleanup);

export { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
