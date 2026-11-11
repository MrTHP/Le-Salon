/* Le Salon — landing page behaviour.
   No dependencies, no trackers, no cookies. */
(() => {
  "use strict";

  const html = document.documentElement;
  const fallbackRelease = {
    tag: "v1.9.0",
    name: "Le-Salon-1.9.0-x86_64.AppImage",
    size: 11991544,
    digest: "sha256:c0528c04f3b9ef5756263edbe185283e40ed0c0ff71c477d5f861a12719a0fe8",
    url: "https://github.com/MrTHP/Le-Salon/releases/latest",
  };
  let release = { ...fallbackRelease };
  let currentLang = "fr";

  const safeStorage = {
    get(key) {
      try {
        return window.localStorage.getItem(key);
      } catch {
        return null;
      }
    },
    set(key, value) {
      try {
        window.localStorage.setItem(key, value);
      } catch {
        /* private mode: ignore */
      }
    },
  };

  const formatSize = (bytes, lang) => {
    const mb = bytes / (1024 * 1024);
    const value = mb >= 10 ? mb.toFixed(1) : mb.toFixed(2);
    return lang === "fr" ? `${value.replace(".", ",")} Mo` : `${value} MB`;
  };

  const renderRelease = () => {
    const downloadUrl = release.url || fallbackRelease.url;
    const digest = (release.digest || "").replace(/^sha256:/i, "");

    document.querySelectorAll("[data-download]").forEach((link) => {
      link.href = downloadUrl;
    });
    document.querySelectorAll("[data-version]").forEach((el) => {
      el.textContent = release.tag || fallbackRelease.tag;
    });
    document.querySelectorAll("[data-size]").forEach((el) => {
      el.textContent = formatSize(release.size || fallbackRelease.size, currentLang);
    });
    document.querySelectorAll("[data-sha]").forEach((el) => {
      el.textContent = digest || "voir la release GitHub";
    });
    document.querySelectorAll("[data-command]").forEach((el) => {
      const name = release.name || fallbackRelease.name;
      el.textContent = `chmod +x ${name}\n./${name}`;
    });
  };

  const setLang = (lang) => {
    currentLang = lang === "en" ? "en" : "fr";
    html.dataset.lang = currentLang;
    html.lang = currentLang;
    safeStorage.set("le-salon-lang", currentLang);
    document.title =
      currentLang === "fr"
        ? "Le Salon — IRC, enfin beau."
        : "Le Salon — IRC, finally beautiful.";

    document.querySelectorAll("[data-set-lang]").forEach((button) => {
      button.setAttribute(
        "aria-pressed",
        String(button.dataset.setLang === currentLang),
      );
    });
    renderRelease();
  };

  const detectLang = () => {
    const stored = safeStorage.get("le-salon-lang");
    if (stored === "fr" || stored === "en") {
      return stored;
    }
    const browserLang = (navigator.language || "fr").toLowerCase();
    return browserLang.startsWith("fr") ? "fr" : "en";
  };

  document.querySelectorAll("[data-set-lang]").forEach((button) => {
    button.addEventListener("click", () => setLang(button.dataset.setLang));
  });

  const loadRelease = async () => {
    try {
      const response = await fetch(
        "https://api.github.com/repos/MrTHP/Le-Salon/releases/latest",
        {
          headers: { Accept: "application/vnd.github+json" },
          cache: "no-store",
        },
      );
      if (!response.ok) {
        return;
      }
      const data = await response.json();
      const asset = (data.assets || []).find((item) =>
        /\.AppImage$/i.test(item.name || ""),
      );
      if (!asset) {
        return;
      }
      release = {
        tag: data.tag_name || fallbackRelease.tag,
        name: asset.name || fallbackRelease.name,
        size: asset.size || fallbackRelease.size,
        digest: asset.digest || "",
        url: asset.browser_download_url || fallbackRelease.url,
      };
      renderRelease();
    } catch {
      /* Offline, rate-limited, or API blocked: the fallback stays usable. */
    }
  };

  const copyText = async (text) => {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.className = "copy-fallback";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  };

  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      const target = document.getElementById(button.dataset.copy);
      if (!target) {
        return;
      }
      try {
        await copyText(target.textContent || "");
        button.classList.add("copied");
        const labels = button.querySelectorAll(".lang-fr, .lang-en");
        labels.forEach((label) => {
          if (!label.dataset.original) {
            label.dataset.original = label.textContent;
          }
          label.textContent =
            label.classList.contains("lang-fr") ? "Copié !" : "Copied!";
        });
        window.setTimeout(() => {
          labels.forEach((label) => {
            if (label.dataset.original) {
              label.textContent = label.dataset.original;
            }
          });
          button.classList.remove("copied");
        }, 1800);
      } catch {
        /* Clipboard denied: keep the text selectable. */
      }
    });
  });

  const initReveal = () => {
    const items = Array.from(document.querySelectorAll(".reveal"));
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduced || !("IntersectionObserver" in window)) {
      items.forEach((item) => item.classList.add("visible"));
      return;
    }
    document.body.classList.add("reveal-ready");
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            entry.target.classList.add("visible");
            observer.unobserve(entry.target);
          }
        });
      },
      { rootMargin: "0px 0px -6% 0px", threshold: 0.08 },
    );
    items.forEach((item) => observer.observe(item));
  };

  const initHeader = () => {
    const header = document.getElementById("site-header");
    if (!header) {
      return;
    }
    const update = () => {
      header.classList.toggle("scrolled", window.scrollY > 12);
    };
    update();
    window.addEventListener("scroll", update, { passive: true });
  };

  document.querySelectorAll("[data-year]").forEach((el) => {
    el.textContent = String(new Date().getFullYear());
  });

  const queryLang = new URLSearchParams(window.location.search).get("lang");
  setLang(queryLang === "fr" || queryLang === "en" ? queryLang : detectLang());
  renderRelease();
  loadRelease();
  initReveal();
  initHeader();
})();
