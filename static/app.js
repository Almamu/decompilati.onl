// Progressive enhancement for edge-cached pages: relative times, the user menu and voting.
// Pages work without this script; it only adds per-visitor bits.
(function () {
  "use strict";

  var units = [["year", 31536000], ["month", 2592000], ["week", 604800], ["day", 86400], ["hour", 3600], ["minute", 60]];
  var rtf = window.Intl && Intl.RelativeTimeFormat ? new Intl.RelativeTimeFormat("en", { numeric: "auto" }) : null;

  function relTimes() {
    var now = Date.now() / 1000;
    document.querySelectorAll("time.ago").forEach(function (el) {
      var t = Date.parse(el.getAttribute("datetime")) / 1000;
      if (!t || !rtf) return;
      var diff = t - now;
      for (var i = 0; i < units.length; i++) {
        if (Math.abs(diff) >= units[i][1]) {
          el.textContent = rtf.format(Math.round(diff / units[i][1]), units[i][0]);
          return;
        }
      }
      el.textContent = "just now";
    });
  }

  function el(tag, attrs, text) {
    var e = document.createElement(tag);
    for (var k in attrs) e.setAttribute(k, attrs[k]);
    if (text) e.textContent = text;
    return e;
  }

  function loginUrl() {
    return "/auth/github?next=" + encodeURIComponent(location.pathname);
  }

  function renderUser(user) {
    var auth = document.getElementById("auth");
    if (!auth) return;
    auth.textContent = "";
    var dash = el("a", { href: "/dashboard", class: "user" });
    if (user.avatar_url) dash.appendChild(el("img", { src: user.avatar_url + "&s=48", alt: "", width: "24", height: "24" }));
    dash.appendChild(document.createTextNode(user.login));
    auth.appendChild(dash);
    var form = el("form", { method: "post", action: "/auth/logout", class: "inline" });
    form.appendChild(el("button", { type: "submit", class: "btn btn-small btn-ghost" }, "Log out"));
    auth.appendChild(form);
  }

  // Explains a failed vote right under the widget, so it's clear why it didn't count.
  function showVoteError(widget, message, offerLogin) {
    var box = widget.querySelector(".vote-error");
    if (!box) {
      box = el("div", { class: "vote-error", role: "alert" });
      widget.appendChild(box);
    }
    box.textContent = message;
    if (offerLogin) {
      box.appendChild(document.createTextNode(" "));
      box.appendChild(el("a", { href: loginUrl() }, "Log in"));
    }
    clearTimeout(widget._errTimer);
    widget._errTimer = setTimeout(function () { hideVoteError(widget); }, 8000);
  }

  function hideVoteError(widget) {
    var box = widget.querySelector(".vote-error");
    if (box) box.remove();
  }

  function setVoteState(widget, value) {
    widget.dataset.value = String(value);
    widget.querySelector(".up").classList.toggle("on", value === 1);
    widget.querySelector(".down").classList.toggle("on", value === -1);
  }

  function initVotes(me) {
    document.querySelectorAll(".vote").forEach(function (w) {
      var id = w.dataset.project;
      var up = w.querySelector(".up"), down = w.querySelector(".down");
      up.disabled = down.disabled = false;
      if (!me) {
        up.onclick = down.onclick = function () { location.href = loginUrl(); };
        up.title = down.title = "Log in to vote";
        return;
      }
      setVoteState(w, (me.votes && me.votes[id]) || 0);
      function cast(v) {
        var current = parseInt(w.dataset.value || "0", 10);
        var next = current === v ? 0 : v;
        up.disabled = down.disabled = true;
        fetch("/api/v1/vote", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ project_id: parseInt(id, 10), value: next })
        })
          .then(function (r) {
            return r.json().then(
              function (j) { return { ok: r.ok, status: r.status, body: j }; },
              function () { return { ok: false, status: r.status, body: {} }; }
            );
          })
          .then(function (res) {
            if (!res.ok) {
              var msg = res.body.error || ("Your vote couldn't be saved (error " + res.status + ").");
              showVoteError(w, msg, res.status === 401);
              return;
            }
            hideVoteError(w);
            // Keep the same project in every widget on the page in sync.
            document.querySelectorAll('.vote[data-project="' + id + '"]').forEach(function (o) {
              o.querySelector(".score").textContent = res.body.score;
              setVoteState(o, next);
            });
          })
          .catch(function () {
            showVoteError(w, "Your vote couldn't be sent. Check your connection and try again.", false);
          })
          .finally(function () { up.disabled = down.disabled = false; });
      }
      up.onclick = function () { cast(1); };
      down.onclick = function () { cast(-1); };
    });
  }

  function init() {
    relTimes();
    // Only logged-in visitors (who carry the "li" hint cookie) need to hit the origin.
    if (!/(?:^|;\s*)li=1/.test(document.cookie)) { initVotes(null); return; }
    fetch("/api/me", { credentials: "same-origin" })
      .then(function (r) { return r.json(); })
      .then(function (me) {
        if (me.user) { renderUser(me.user); initVotes(me); }
        else { document.cookie = "li=; Max-Age=0; Path=/"; initVotes(null); }
      })
      .catch(function () { initVotes(null); });
  }

  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);
  else init();
})();
