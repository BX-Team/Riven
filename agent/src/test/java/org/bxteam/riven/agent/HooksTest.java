package org.bxteam.riven.agent;

import static org.junit.jupiter.api.Assertions.*;

import java.net.URI;
import java.net.URL;
import org.junit.jupiter.api.Test;

class HooksTest {
    private static final String SESSION = "https://sessionserver.mojang.com/session/minecraft/";

    @Test
    void onlyOfflineCallsLeaveMojang() throws Exception {
        System.setProperty(Hooks.API, "https://api.example.test/v1/skins/");
        System.clearProperty(Hooks.JOIN);
        // A Microsoft account's join carries its token: it must keep going to Mojang.
        assertEquals(SESSION + "join", Hooks.url(URI.create(SESSION + "join").toURL()).toString());
        System.setProperty(Hooks.JOIN, "true");
        assertEquals("https://api.example.test/v1/skins/session/minecraft/join", Hooks.url(URI.create(SESSION + "join").toURL()).toString());

        String offline = "b50ad385829d3141a2167e7d7539ba7f";
        String mojang = "069a79f444e94726a5befca90e38aaf5";
        assertEquals(
                "https://api.example.test/v1/skins/session/minecraft/profile/" + offline + "?unsigned=false",
                Hooks.url(URI.create(SESSION + "profile/" + offline + "?unsigned=false").toURL()).toString());
        assertEquals(SESSION + "profile/" + mojang, Hooks.url(URI.create(SESSION + "profile/" + mojang).toURL()).toString());
        assertEquals("https://api.mojang.com/profiles/minecraft", Hooks.url(URI.create("https://api.mojang.com/profiles/minecraft").toURL()).toString());
        System.clearProperty(Hooks.JOIN);
    }
}
