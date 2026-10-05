/*
 * A minimal ticket gate: verifies one QR code against one trusted key.
 *
 *     verify <public-key-hex> <base45-text>
 *
 * Exits 0 and prints the ticket id when the ticket is authentic and currently valid,
 * otherwise prints the rejection (e.g. "ピンポーン🔔 BadSignature") and exits 1.
 * Everything beyond this — re-entry rules, zones, logging — is up to your gate.
 */
#include <stdio.h>
#include <string.h>

#include "kaisatsu.h"

static int parse_hex_key(const char *hex, KaisatsuKey *key) {
    if (strlen(hex) != 64) return 0;
    for (size_t i = 0; i < 32; i++) {
        unsigned int byte;
        if (sscanf(hex + 2 * i, "%2x", &byte) != 1) return 0;
        key->public_key[i] = (uint8_t)byte;
    }
    return 1;
}

static void print_uuid(const uint8_t id[16]) {
    for (size_t i = 0; i < 16; i++) {
        if (i == 4 || i == 6 || i == 8 || i == 10) putchar('-');
        printf("%02x", id[i]);
    }
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s <public-key-hex> <base45-text>\n", argv[0]);
        return 2;
    }

    KaisatsuKey key;
    if (!parse_hex_key(argv[1], &key) || kaisatsu_key_check(&key, NULL) != KAISATSU_STATUS_OK) {
        fprintf(stderr, "invalid public key\n");
        return 2;
    }

    uint8_t bytes[KAISATSU_MAX_TICKET_LEN];
    size_t len = 0;
    KaisatsuStatus status = kaisatsu_base45_decode(argv[2], strlen(argv[2]), bytes, sizeof bytes, &len);
    if (status != KAISATSU_STATUS_OK) {
        printf("%s\n", kaisatsu_status_message(status));
        return 1;
    }

    KaisatsuTicket ticket;
    status = kaisatsu_verify(&key, 1, bytes, len, &ticket);
    if (status != KAISATSU_STATUS_OK) {
        printf("%s\n", kaisatsu_status_message(status));
        return 1;
    }

    printf("OK ticket ");
    print_uuid(ticket.ticket_id);
    printf(" issued by %.*s", (int)ticket.issuer_len, (const char *)ticket.issuer);

    const uint8_t *zone;
    size_t zone_len;
    if (kaisatsu_ticket_extension(&ticket, 0x80, &zone, &zone_len)) {
        printf(" zone %.*s", (int)zone_len, (const char *)zone);
    }
    printf("\n");
    return 0;
}
