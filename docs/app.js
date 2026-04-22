const lang = document.body.dataset.siteLang === "es" ? "es" : "en";

document.documentElement.lang = lang;

if (document.body.dataset.titleEn && document.body.dataset.titleEs) {
  document.title = lang === "es" ? document.body.dataset.titleEs : document.body.dataset.titleEn;
}

document.querySelectorAll("[data-page-link]").forEach((link) => {
  const target = link.dataset.pageLink;
  link.href = target;
  if (document.body.dataset.page === target) link.classList.add("is-active");
});

document.querySelectorAll("[data-lang-switch]").forEach((link) => {
  const targetLang = link.dataset.langSwitch;
  link.href = `../${targetLang}/${document.body.dataset.page}`;
  link.classList.toggle("is-active", targetLang === lang);
});

const i18n = {
  en: {
    downloadFor: "Download for",
    downloadNote: "Not your setup? Try another build:",
    chooseBuild: "Choose Build",
    accel: "Select acceleration option:",
    openblasAll: "all variants include openblas",
    openblasCpu: "includes openblas",
    cudaHint: "NVIDIA cards",
    vulkanHint: "AMD cards"
  },
  es: {
    downloadFor: "Descargar para",
    downloadNote: "No es tu equipo? Prueba otra compilacion:",
    chooseBuild: "Elegir compilacion",
    accel: "Selecciona la opcion de aceleracion:",
    openblasAll: "todas las variantes incluyen openblas",
    openblasCpu: "incluye openblas",
    cudaHint: "tarjetas NVIDIA",
    vulkanHint: "tarjetas AMD"
  }
};

const variants = [
  { key: "windows-x86", os: "Windows", arch: "x86", href: "#windows-x86-download" },
  { key: "macos-amd64", os: "macOS", arch: "amd64", href: "#macos-amd64-download" },
  { key: "linux-amd64", os: "Linux", arch: "amd64", href: "#linux-amd64-download" },
  { key: "linux-arm64", os: "Linux", arch: "arm64", href: "#linux-arm64-download" }
];

const runtimeOptions = {
  "windows-x86": [
    { label: "CPU", href: "#windows-x86-cpu-download" },
    { label: "CUDA", href: "#windows-x86-cuda-download" },
    { label: "VULKAN", href: "#windows-x86-vulkan-download" }
  ],
  "linux-amd64": [
    { label: "CPU", href: "#linux-amd64-cpu-download" },
    { label: "CUDA", href: "#linux-amd64-cuda-download" },
    { label: "VULKAN", href: "#linux-amd64-vulkan-download" }
  ],
  "linux-arm64": [
    { label: "CPU", href: "#linux-arm64-cpu-download" },
    { label: "VULKAN", href: "#linux-arm64-vulkan-download" }
  ]
};

function detectVariant() {
  const platform = (navigator.userAgentData?.platform || navigator.platform || "").toLowerCase();
  const ua = (navigator.userAgent || "").toLowerCase();
  const uaDataArch = (navigator.userAgentData?.architecture || "").toLowerCase();

  let os = "Linux";
  if (platform.includes("win") || ua.includes("windows")) os = "Windows";
  else if (platform.includes("mac") || ua.includes("mac os")) os = "macOS";
  else if (platform.includes("linux") || ua.includes("linux")) os = "Linux";

  let arch = "amd64";
  if (os === "Windows") {
    arch = "x86";
  } else {
    const armHint =
      uaDataArch.includes("arm") ||
      platform.includes("arm") ||
      ua.includes("aarch64") ||
      ua.includes("arm64");
    arch = armHint ? "arm64" : "amd64";
  }

  return variants.find((variant) => variant.os === os && variant.arch === arch) || variants[2];
}

function closeDownloadModal() {
  const modal = document.getElementById("download-modal");
  if (!modal) return;
  modal.classList.remove("open");
  modal.setAttribute("aria-hidden", "true");
}

function openDownloadModal(variant) {
  const modal = document.getElementById("download-modal");
  if (!modal) return;

  const modalTitle = document.getElementById("modal-title");
  const modalSubtitle = document.getElementById("modal-subtitle");
  const modalOptions = document.getElementById("modal-options");
  const modalNote = document.getElementById("modal-note");
  const copy = i18n[lang];
  const options = runtimeOptions[variant.key] || [];

  modalTitle.textContent = `${copy.chooseBuild}: ${variant.os} (${variant.arch})`;
  modalSubtitle.textContent = copy.accel;
  modalNote.textContent = copy.openblasAll;
  modalOptions.innerHTML = "";

  options.forEach((option) => {
    const item = document.createElement("a");
    item.className = "modal-option";
    item.href = option.href;

    const label = document.createElement("span");
    label.className = "modal-option-label";
    label.textContent = option.label;
    item.appendChild(label);

    const hint = document.createElement("span");
    hint.className = "modal-option-hint";
    if (option.label === "CPU") hint.textContent = copy.openblasCpu;
    if (option.label === "CUDA") hint.textContent = copy.cudaHint;
    if (option.label === "VULKAN") hint.textContent = copy.vulkanHint;
    if (hint.textContent) item.appendChild(hint);

    modalOptions.appendChild(item);
  });

  modal.classList.add("open");
  modal.setAttribute("aria-hidden", "false");
}

function bindVariantLink(link, variant) {
  link.href = variant.href;
  link.addEventListener("click", (event) => {
    if (variant.os === "Linux" || variant.os === "Windows") {
      event.preventDefault();
      openDownloadModal(variant);
    }
  });
}

const primaryBtn = document.getElementById("primary-download");
const note = document.getElementById("download-note");
const others = document.getElementById("other-downloads");

if (primaryBtn && note && others) {
  const primary = detectVariant();
  const copy = i18n[lang];

  primaryBtn.textContent = `${copy.downloadFor} ${primary.os} (${primary.arch})`;
  bindVariantLink(primaryBtn, primary);
  note.textContent = copy.downloadNote;

  variants
    .filter((variant) => variant.key !== primary.key)
    .forEach((variant) => {
      const link = document.createElement("a");
      link.className = "variant-link";
      link.textContent = `${variant.os} (${variant.arch})`;
      bindVariantLink(link, variant);
      others.appendChild(link);
    });

  document.getElementById("modal-close")?.addEventListener("click", closeDownloadModal);
  document.getElementById("download-modal")?.addEventListener("click", (event) => {
    if (event.target.id === "download-modal") closeDownloadModal();
  });
}

function openVideoModal(videoId) {
  const modal = document.getElementById("video-modal");
  const frame = document.getElementById("video-modal-frame");
  if (!modal || !frame) return;
  frame.src = `https://www.youtube.com/embed/${videoId}?autoplay=1`;
  modal.classList.add("open");
  modal.setAttribute("aria-hidden", "false");
}

function closeVideoModal() {
  const modal = document.getElementById("video-modal");
  const frame = document.getElementById("video-modal-frame");
  if (!modal || !frame) return;
  frame.src = "";
  modal.classList.remove("open");
  modal.setAttribute("aria-hidden", "true");
}

document.querySelectorAll(".demo-thumb").forEach((thumb) => {
  thumb.addEventListener("click", () => openVideoModal(thumb.dataset.videoId));
});

document.getElementById("video-modal-close")?.addEventListener("click", closeVideoModal);
document.getElementById("video-modal")?.addEventListener("click", (event) => {
  if (event.target.id === "video-modal") closeVideoModal();
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    closeDownloadModal();
    closeVideoModal();
  }
});
