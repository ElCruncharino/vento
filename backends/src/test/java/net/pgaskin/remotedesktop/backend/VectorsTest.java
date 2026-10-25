// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

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
 * The test vectors in {@code backends/vectors/}: one set of cases for each
 * rule, so that the copy here cannot drift from them without one of these
 * failing.
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

    @Test
    public void splitAgreesWithTheVectors() throws IOException {
        for (Map<String, Object> c : cases(vectors("credentials.json"), "split_domain")) {
            final String[] split = RdpIdentity.split(str(c, "typed"));
            assertEquals(c.toString(), str(c, "domain"), split[0]);
            assertEquals(c.toString(), str(c, "user"), split[1]);
        }
    }

    /** A vector with a domain typed beside the name is not this side's: the prompt has no box for one. */
    @Test
    public void rdpAnswersAgreeWithTheVectors() throws IOException {
        int checked = 0;
        for (Map<String, Object> c : cases(vectors("credentials.json"), "rdp_identity")) {
            if (c.containsKey("answer_domain") && !str(c, "answer_domain").isEmpty()) {
                continue;
            }
            final String[] answer = RdpIdentity.answer(str(c, "answer_user"),
                    str(c, "session_domain"), str(c, "session_user"));
            assertEquals(c.toString(), str(c, "domain"), answer[0]);
            assertEquals(c.toString(), str(c, "user"), answer[1]);
            checked++;
        }
        assertTrue(checked > 0);
    }

    @Test
    public void knownHostKeysAndFingerprintsAgreeWithTheVectors() throws IOException {
        final Map<String, Object> file = vectors("known_hosts.json");
        for (Map<String, Object> c : cases(file, "key")) {
            assertEquals(c.toString(), str(c, "key"), KnownHosts.key(str(c, "address")));
        }
        for (Map<String, Object> c : cases(file, "canonical")) {
            assertEquals(c.toString(), str(c, "canonical"),
                    KnownHosts.canonical(str(c, "fingerprint")));
        }
    }

    @Test
    public void knownHostVerdictsAgreeWithTheVectors() throws IOException {
        for (Map<String, Object> c : cases(vectors("known_hosts.json"), "verdict")) {
            final KnownHosts.Verdict verdict = KnownHosts.verdict(strings(c, "pins"),
                    str(c, "address"), str(c, "fingerprint"));
            if (str(c, "verdict").equals("matches")) {
                assertTrue(c.toString(), verdict.matches());
                continue;
            }
            final Prompt.Trust.Identity want = switch (str(c, "identity")) {
                case "new" -> Prompt.Trust.Identity.NEW;
                case "matches_another" -> Prompt.Trust.Identity.MATCHES_ANOTHER;
                case "changed" -> Prompt.Trust.Identity.CHANGED;
                default -> throw new AssertionError(c.toString());
            };
            assertEquals(c.toString(), want, verdict.identity());
            final String other = verdict.matchingAddress();
            assertEquals(c.toString(), str(c, "matching_address"), other == null ? "" : other);
        }
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
