// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The shared test vectors in {@code backends/vectors/}, which
 * {@code backends/session/tests/vectors.rs} reads too: one set of cases for a
 * rule both sides keep, so that the two copies cannot drift apart without one
 * of these failing.
 *
 * <p>One rule is left with a Java copy: {@link BackendOption#settle}, which the
 * option screens run as somebody edits. The credentials and known-hosts
 * cases are the Rust side's alone, written in that file, since the rules
 * they hold are only there.
 */
public class VectorsTest {

    @SuppressWarnings("unchecked")
    private static Map<String, Object> vectors(String name) throws IOException {
        // A unit test runs in its module's directory. readAllBytes rather than
        // readString: a unit test compiles against android.jar.
        return (Map<String, Object>) Json.parse(new String(
                Files.readAllBytes(Path.of("vectors", name)), StandardCharsets.UTF_8));
    }

    @SuppressWarnings("unchecked")
    private static List<Map<String, Object>> cases(Map<String, Object> file, String section) {
        return (List<Map<String, Object>>) file.get(section);
    }

    private static String str(Map<String, Object> c, String key) {
        final Object value = c.get(key);
        if (!(value instanceof String s)) {
            throw new AssertionError("no " + key + " in " + c);
        }
        return s;
    }

    @SuppressWarnings("unchecked")
    private static Map<String, String> strings(Map<String, Object> c, String key) {
        return new HashMap<>((Map<String, String>) c.get(key));
    }

    @SuppressWarnings("unchecked")
    private static BackendOption spec(Map<String, Object> s) {
        final String key = str(s, "key");
        if (str(s, "type").equals("bool")) {
            return BackendOption.bool(key, key, null, Boolean.parseBoolean(str(s, "default")),
                    BackendOption.Scope.CONNECTION, false);
        }
        final List<BackendOption.Choice> choices = new ArrayList<>();
        for (Map<String, Object> c : (List<Map<String, Object>>) s.get("choices")) {
            final String value = str(c, "value");
            choices.add(new BackendOption.Choice(value, value, null, (String) c.get("requires")));
        }
        return BackendOption.choice(key, key, null, str(s, "default"),
                BackendOption.Scope.CONNECTION, false, choices.toArray(new BackendOption.Choice[0]));
    }

    @Test
    @SuppressWarnings("unchecked")
    public void settleAgreesWithTheVectors() throws IOException {
        final Map<String, Object> file = vectors("options.json");
        final List<BackendOption> specs = new ArrayList<>();
        for (Map<String, Object> s : cases(file, "specs")) {
            specs.add(spec(s));
        }
        for (Map<String, Object> c : cases(file, "settle")) {
            final Map<String, String> values = strings(c, "values");
            final List<String> changed = BackendOption.settle(specs, o -> values.get(o.key()),
                    key -> Boolean.parseBoolean(values.get(key)),
                    (o, v) -> values.put(o.key(), v));
            assertEquals(c.toString(), c.get("changed"), changed);
            assertEquals(c.toString(), strings(c, "settled"), values);
        }
    }
}
