import { expect, test } from "bun:test";
import { escapePath, pathsFromUris, safeName } from "./drop";

test("Finder's file URLs become paths; other lines and URLs don't", () => {
  expect(pathsFromUris("file:///Users/me/Screen%20Shot%201.png\r\n# comment\nhttps://x.dev/a.png\nfile:///tmp/a%23b.txt")).toEqual([
    "/Users/me/Screen Shot 1.png",
    "/tmp/a#b.txt",
  ]);
  expect(pathsFromUris("")).toEqual([]);
});

test("paths are escaped the way Terminal pastes them", () => {
  expect(escapePath("/Users/me/Screen Shot (1).png")).toBe("/Users/me/Screen\\ Shot\\ \\(1\\).png");
  expect(escapePath("/tmp/it's&$x.txt")).toBe("/tmp/it\\'s\\&\\$x.txt");
  expect(escapePath("/tmp/plain-file_1.png")).toBe("/tmp/plain-file_1.png");
});

test("a dropped file's name is kept safe for a header and a file system", () => {
  expect(safeName("Screen Shot 1.png")).toBe("Screen_Shot_1.png");
  expect(safeName("../../etc/passwd")).toBe(".._.._etc_passwd");
  expect(safeName("")).toBe("file");
});
