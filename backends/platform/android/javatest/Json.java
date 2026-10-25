// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Just enough JSON for the test vectors: objects as maps in their order,
 * arrays as lists, strings, numbers as doubles, booleans and null.
 *
 * <p>Written out because a unit test compiles against android.jar, whose
 * {@code org.json} is a set of stubs that throw, and adding the real one
 * makes it a question of which jar comes first on a class path.
 */
final class Json {
    private final String text;
    private int at;

    private Json(String text) {
        this.text = text;
    }

    static Object parse(String text) {
        final Json json = new Json(text);
        final Object value = json.value();
        json.space();
        if (json.at != text.length()) {
            throw json.error("trailing text");
        }
        return value;
    }

    private IllegalArgumentException error(String what) {
        return new IllegalArgumentException(what + " at " + at);
    }

    private void space() {
        while (at < text.length() && Character.isWhitespace(text.charAt(at))) {
            at++;
        }
    }

    private boolean eat(char c) {
        space();
        if (at < text.length() && text.charAt(at) == c) {
            at++;
            return true;
        }
        return false;
    }

    private void expect(char c) {
        if (!eat(c)) {
            throw error("expected " + c);
        }
    }

    private Object value() {
        space();
        if (at >= text.length()) {
            throw error("unexpected end");
        }
        final char c = text.charAt(at);
        if (c == '{') {
            at++;
            final Map<String, Object> object = new LinkedHashMap<>();
            if (eat('}')) {
                return object;
            }
            do {
                space();
                final String key = string();
                expect(':');
                object.put(key, value());
            } while (eat(','));
            expect('}');
            return object;
        }
        if (c == '[') {
            at++;
            final List<Object> array = new ArrayList<>();
            if (eat(']')) {
                return array;
            }
            do {
                array.add(value());
            } while (eat(','));
            expect(']');
            return array;
        }
        if (c == '"') {
            return string();
        }
        for (String word : new String[]{"true", "false", "null"}) {
            if (text.startsWith(word, at)) {
                at += word.length();
                return word.equals("null") ? null : Boolean.valueOf(word);
            }
        }
        final int start = at;
        while (at < text.length() && "+-0123456789.eE".indexOf(text.charAt(at)) >= 0) {
            at++;
        }
        if (start == at) {
            throw error("unexpected " + c);
        }
        return Double.parseDouble(text.substring(start, at));
    }

    private String string() {
        if (at >= text.length() || text.charAt(at) != '"') {
            throw error("expected a string");
        }
        at++;
        final StringBuilder out = new StringBuilder();
        while (true) {
            if (at >= text.length()) {
                throw error("unterminated string");
            }
            final char c = text.charAt(at++);
            if (c == '"') {
                return out.toString();
            }
            if (c != '\\') {
                out.append(c);
                continue;
            }
            final char e = text.charAt(at++);
            switch (e) {
                case '"', '\\', '/' -> out.append(e);
                case 'b' -> out.append('\b');
                case 'f' -> out.append('\f');
                case 'n' -> out.append('\n');
                case 'r' -> out.append('\r');
                case 't' -> out.append('\t');
                case 'u' -> {
                    out.append((char) Integer.parseInt(text.substring(at, at + 4), 16));
                    at += 4;
                }
                default -> throw error("bad escape \\" + e);
            }
        }
    }
}
