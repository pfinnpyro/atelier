// The world a form script sees: its fields as plain values, a few Acrobat
// objects that only touch those values, and Acrobat's common AF helpers.
// There is nothing here that reaches the file, the app, the network or the
// computer.
"use strict";
var __changed = {};
var __alerts = [];
var __shown = {};
function __num(s) {
  if (typeof s !== "string") return s;
  var t = s.trim();
  if (t !== "" && !isNaN(Number(t))) return Number(t);
  return s;
}
function __names(prefix) {
  var out = [];
  for (var n in __fields) {
    if (n === prefix || n.indexOf(prefix + ".") === 0) out.push(n);
  }
  return out;
}
function Field(name) { this.name = name; }
Object.defineProperty(Field.prototype, "value", {
  get: function () {
    var f = __fields[this.name];
    return f === undefined ? "" : __num(f.value);
  },
  set: function (v) {
    var f = __fields[this.name];
    if (f === undefined) return;
    var s = v === null || v === undefined ? "" : String(v);
    f.value = s;
    __changed[this.name] = s;
  },
});
Object.defineProperty(Field.prototype, "valueAsString", {
  get: function () {
    var f = __fields[this.name];
    return f === undefined ? "" : f.value;
  },
});
Object.defineProperty(Field.prototype, "type", {
  get: function () {
    var f = __fields[this.name];
    return f === undefined ? "" : f.kind;
  },
});
Field.prototype.getArray = function () {
  var names = __names(this.name).filter(function (n) { return n !== this.name; }, this);
  return names.map(function (n) { return new Field(n); });
};
Field.prototype.setFocus = function () {};
Field.prototype.isBoxChecked = function () {
  var v = this.valueAsString;
  return v !== "" && v !== "Off";
};
// Showing and hiding fields is passed back to the viewer.
function __hide(name, hidden) {
  hidden = !!hidden;
  var all = __names(name);
  if (all.length === 0) return;
  all.forEach(function (n) { __fields[n].hidden = hidden; });
  __shown[name] = hidden;
}
Object.defineProperty(Field.prototype, "hidden", {
  get: function () {
    var f = __fields[this.name];
    return f !== undefined && f.hidden;
  },
  set: function (v) { __hide(this.name, v); },
});
Object.defineProperty(Field.prototype, "display", {
  get: function () { return this.hidden ? 1 : 0; },
  set: function (v) { __hide(this.name, v === 1 || v === 3); },
});
// Other display properties are accepted and ignored.
["readonly", "textColor", "fillColor", "required",
 "borderStyle", "textSize", "textFont", "userName", "defaultValue"].forEach(function (p) {
  var store = {};
  Object.defineProperty(Field.prototype, p, {
    get: function () { return store[this.name]; },
    set: function (v) { store[this.name] = v; },
  });
});

function getField(name) {
  name = String(name);
  if (name in __fields || __names(name).length > 0) return new Field(name);
  return null;
}
function calculateNow() {}
function resetForm() {}
var numFields = Object.keys(__fields).length;
function getNthFieldName(n) { return Object.keys(__fields)[n] || null; }

var app = {
  alert: function (m) {
    var msg = m !== null && typeof m === "object" && "cMsg" in m ? m.cMsg : m;
    __alerts.push(String(msg));
    return 1;
  },
  beep: function () {},
  platform: "WIN",
  viewerType: "Reader",
  viewerVariation: "Reader",
  viewerVersion: 11,
  language: "ENU",
};
var console = { println: function () {}, show: function () {}, clear: function () {} };
var display = { visible: 0, hidden: 1, noPrint: 2, noView: 3 };
var color = {
  transparent: ["T"], black: ["G", 0], white: ["G", 1], red: ["RGB", 1, 0, 0],
  green: ["RGB", 0, 1, 0], blue: ["RGB", 0, 0, 1], gray: ["G", 0.5],
};

// ----- Acrobat's AF helpers ---------------------------------------------

function AFMakeNumber(s) {
  if (typeof s === "number") return s;
  if (s === null || s === undefined) return null;
  var t = String(s).trim().replace(/[^0-9.,\-]/g, "");
  // The last of "." and "," is the decimal point; the other groups digits.
  var comma = t.lastIndexOf(","), dot = t.lastIndexOf(".");
  if (comma > dot) t = t.replace(/\./g, "").replace(",", ".");
  else t = t.replace(/,/g, "");
  if ((t.match(/\./g) || []).length > 1) return null;
  var n = parseFloat(t);
  return isNaN(n) ? null : n;
}

function __fieldNames(list) {
  if (typeof list === "string") list = list.split(",");
  var out = [];
  for (var i = 0; i < list.length; i++) {
    var n = String(list[i]).trim();
    var all = __names(n);
    if (all.length === 0) continue;
    all.forEach(function (x) { if (out.indexOf(x) < 0) out.push(x); });
  }
  return out;
}

function AFSimple_Calculate(op, list) {
  var names = __fieldNames(list);
  var vals = names.map(function (n) { return AFMakeNumber(__fields[n].value) || 0; });
  var r;
  switch (String(op).toUpperCase()) {
    case "SUM": r = vals.reduce(function (a, b) { return a + b; }, 0); break;
    case "PRD": r = vals.reduce(function (a, b) { return a * b; }, 1); break;
    case "AVG": r = vals.length ? vals.reduce(function (a, b) { return a + b; }, 0) / vals.length : 0; break;
    case "MIN": r = vals.length ? Math.min.apply(null, vals) : 0; break;
    case "MAX": r = vals.length ? Math.max.apply(null, vals) : 0; break;
    default: r = 0;
  }
  event.value = r;
}

function __group(s, sep) {
  var parts = s.split(".");
  parts[0] = parts[0].replace(/\B(?=(\d{3})+(?!\d))/g, sep);
  return parts.join(sep === "." ? "," : ".");
}

function AFNumber_Format(nDec, sepStyle, negStyle, currStyle, strCurrency, bPrepend) {
  var n = AFMakeNumber(event.value);
  if (n === null) { event.value = ""; return; }
  var neg = n < 0;
  var s = Math.abs(n).toFixed(nDec);
  if (sepStyle === 0) s = __group(s, ",");
  else if (sepStyle === 2) s = __group(s, ".");
  else if (sepStyle === 1) s = s;
  else if (sepStyle === 3) s = s.replace(".", ",");
  strCurrency = strCurrency || "";
  s = bPrepend ? strCurrency + s : s + strCurrency;
  if (neg) s = negStyle === 2 || negStyle === 3 ? "(" + s + ")" : "-" + s;
  event.value = s;
}

function AFNumber_Keystroke(nDec, sepStyle) {
  if (!event.willCommit) return;
  var v = String(event.value).trim();
  if (v === "") return;
  if (AFMakeNumber(v) === null) {
    app.alert("The value entered does not match the format of the field [ " + event.targetName + " ]");
    event.rc = false;
  }
}

function AFPercent_Format(nDec, sepStyle) {
  var n = AFMakeNumber(event.value);
  if (n === null) { event.value = ""; return; }
  var s = (n * 100).toFixed(nDec);
  if (sepStyle === 0) s = __group(s, ",");
  event.value = s + "%";
}
var AFPercent_Keystroke = AFNumber_Keystroke;

function AFRange_Validate(bGreater, nGreater, bLess, nLess) {
  var v = String(event.value).trim();
  if (v === "") return;
  var n = AFMakeNumber(v);
  if (n === null) return;
  var bad = (bGreater && n < nGreater) || (bLess && n > nLess);
  if (bad) {
    var msg = "The value entered must be";
    if (bGreater) msg += " greater than or equal to " + nGreater;
    if (bGreater && bLess) msg += " and";
    if (bLess) msg += " less than or equal to " + nLess;
    app.alert(msg + ".");
    event.rc = false;
  }
}

function AFSpecial_Format(psf) {
  var d = String(event.value).replace(/\D/g, "");
  if (d === "") return;
  if (psf === 0) event.value = d.slice(0, 5);
  else if (psf === 1) event.value = d.slice(0, 5) + "-" + d.slice(5, 9);
  else if (psf === 2) event.value = d.length >= 10
    ? "(" + d.slice(0, 3) + ") " + d.slice(3, 6) + "-" + d.slice(6, 10) : d.slice(0, 3) + "-" + d.slice(3, 7);
  else if (psf === 3) event.value = d.slice(0, 3) + "-" + d.slice(3, 5) + "-" + d.slice(5, 9);
}
function AFSpecial_Keystroke(psf) {}
function AFSpecial_KeystrokeEx(mask) {}
function AFDate_FormatEx(f) {}
function AFDate_KeystrokeEx(f) {}
function AFDate_Format(f) {}
function AFDate_Keystroke(f) {}
function AFTime_Format(f) {}
function AFTime_Keystroke(f) {}

var util = {
  printf: function (fmt) {
    var args = arguments, i = 1;
    return String(fmt).replace(/%(,\d)?(\.(\d+))?([dfsx%])/g, function (m, sep, dot, dec, kind) {
      if (kind === "%") return "%";
      var a = args[i++];
      if (kind === "d") return String(Math.trunc(AFMakeNumber(a) || 0));
      if (kind === "f") return (AFMakeNumber(a) || 0).toFixed(dec === undefined ? 6 : Number(dec));
      if (kind === "x") return (Math.trunc(AFMakeNumber(a) || 0)).toString(16);
      return String(a);
    });
  },
  printd: function (fmt, d) { return d instanceof Date ? d.toISOString().slice(0, 10) : ""; },
  scand: function (fmt, s) { var d = new Date(s); return isNaN(d) ? null : d; },
};
