// Where things live. With REPO_URL null the source links say the
// repository isn't public instead of linking to a 404.
const THRESHOLD = {
  version: "0.1.0",
  REPO_URL: "https://github.com/thirdbearsolutions/threshold.app",
  // Builds. A null url means no build yet: "Build from source" while the
  // repository is public, "Coming soon" otherwise.
  builds: {
    "mac-arm64": "downloads/Threshold_0.1.0_aarch64.dmg",
    "mac-x64": null,
    "linux": null,
    "windows": null,
  },
};

function detectPlatform() {
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes("win")) return "windows";
  if (ua.includes("android") || ua.includes("iphone") || ua.includes("ipad")) return "phone";
  if (ua.includes("mac")) {
    try {
      const gl = document.createElement("canvas").getContext("webgl");
      const info = gl && gl.getExtension("WEBGL_debug_renderer_info");
      const r = info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL).toLowerCase() : "";
      if (r.includes("intel")) return "mac-x64";
    } catch (e) {}
    return "mac-arm64";
  }
  if (ua.includes("linux")) return "linux";
  return null;
}

function wireRepo() {
  document.querySelectorAll("[data-repo]").forEach((el) => {
    const path = el.getAttribute("data-repo") || "";
    if (THRESHOLD.REPO_URL) {
      el.href = THRESHOLD.REPO_URL + path;
      el.classList.remove("pending");
    } else {
      el.removeAttribute("href");
      el.classList.add("pending");
      el.title = "The repository is not public yet";
    }
  });
  document.querySelectorAll("[data-repo-text]").forEach((el) => {
    el.textContent = THRESHOLD.REPO_URL
      ? THRESHOLD.REPO_URL.replace(/^https?:\/\//, "")
      : "github.com/…/threshold";
  });
}

function wireDownloads() {
  const here = detectPlatform();
  document.querySelectorAll("[data-build]").forEach((card) => {
    const key = card.getAttribute("data-build");
    const url = THRESHOLD.builds[key];
    const btn = card.querySelector(".dl-btn");
    if (btn) {
      if (url) {
        btn.href = url;
        btn.classList.remove("pending");
      } else {
        btn.removeAttribute("href");
        btn.classList.add("pending");
        if (THRESHOLD.REPO_URL) {
          btn.textContent = "Build from source";
          btn.href = "guide.html#building-from-source";
        } else {
          btn.textContent = "Coming soon";
        }
      }
    }
    if (key === here) card.classList.add("yours");
  });
  const note = document.getElementById("dl-detected");
  const names = { "mac-arm64": "a Mac with Apple silicon", "mac-x64": "an Intel Mac",
    linux: "Linux", windows: "Windows", phone: "a phone" };
  if (note && here) {
    note.textContent = here === "phone"
      ? "You're on a phone. Threshold runs on your Mac; the phone pairs with it."
      : `Looks like you're on ${names[here]}.`;
  }
  document.querySelectorAll("[data-version]").forEach((el) => (el.textContent = THRESHOLD.version));
}

document.addEventListener("DOMContentLoaded", () => {
  wireDownloads();
  wireRepo();
});
